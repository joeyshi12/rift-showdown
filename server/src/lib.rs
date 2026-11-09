//! HTTP + WebSocket server for Rift Showdown.
//!
//! Routes:
//!   GET /ws           WebSocket game protocol (JSON `ClientMsg`/`ServerMsg`)
//!   GET /cards.json   card database
//!   GET /img?u=URL    same-origin proxy + disk cache for official card images
//!   GET /healthz      liveness
//!   everything else   static client files from the dist directory

pub mod hub;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use hub::Hub;
use rift_shared::cards::CardDb;
use rift_shared::protocol::{ClientMsg, ServerMsg, MAX_MSG_BYTES};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::{SetResponseHeader, SetResponseHeaderLayer};

const IMG_HOST: &str = "https://cmsassets.rgpub.io/";
/// League champion splash art from Riot's Data Dragon CDN (home screen hero). Served as-is.
const SPLASH_PREFIX: &str = "https://ddragon.leagueoflegends.com/cdn/img/champion/splash/";
/// Full-size champion splash paintings, as resized by the League of Legends Wiki (home screen hero).
const WIKI_THUMB_PREFIX: &str = "https://wiki.leagueoflegends.com/en-us/images/thumb/";

/// `<Name>_OriginalSkin_HD.jpg/<W>px-<Name>_OriginalSkin_HD.jpg` with W in 1280/1920/2560.
fn wiki_splash_ok(rest: &str) -> bool {
    let Some((file, thumb)) = rest.split_once('/') else { return false };
    let Some(name) = file.strip_suffix("_OriginalSkin_HD.jpg") else { return false };
    let name_ok = !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' || b == b'.');
    name_ok && ["1280", "1920", "2560"].iter().any(|w| thumb.strip_prefix(w).and_then(|t| t.strip_prefix("px-")) == Some(file))
}

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Mutex<Hub>>,
    pub cards_json: Arc<String>,
    pub img_cache: PathBuf,
    pub http: reqwest::Client,
}

pub struct Config {
    pub data_dir: PathBuf,
    pub dist_dir: PathBuf,
    /// Card image cache directory; defaults to `<data_dir>/imgcache`.
    pub img_cache: Option<PathBuf>,
}

pub fn load_state(cfg: &Config) -> Result<AppState, String> {
    let json = std::fs::read_to_string(cfg.data_dir.join("cards.json")).map_err(|e| format!("reading cards.json: {e}"))?;
    let db = CardDb::from_json(&json)?;
    let img_cache = cfg.img_cache.clone().unwrap_or_else(|| cfg.data_dir.join("imgcache"));
    std::fs::create_dir_all(&img_cache).map_err(|e| format!("creating the image cache {}: {e} (the server needs write access there)", img_cache.display()))?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).user_agent("rift-showdown/0.1").build().map_err(|e| e.to_string())?;
    Ok(AppState {
        hub: Arc::new(Mutex::new(Hub::new(Arc::new(db)))),
        cards_json: Arc::new(json),
        img_cache,
        http,
    })
}

pub fn router(state: AppState, dist: PathBuf) -> Router {
    let index = dist.join("index.html");
    // App files keep their names across builds: browsers and CDNs must revalidate
    // them (cheap 304s via Last-Modified) instead of keeping a stale copy for hours.
    let statics = SetResponseHeader::if_not_present(ServeDir::new(&dist).fallback(ServeFile::new(index)), header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/cards.json", get(cards_handler))
        .route("/img", get(img_handler))
        .route("/healthz", get(|| async { "ok" }))
        .fallback_service(statics)
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY, HeaderValue::from_static("same-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .with_state(state)
}

/// Background housekeeping: queue status ticks and session sweeping.
pub fn spawn_maintenance(state: &AppState) {
    let hub = state.hub.clone();
    tokio::spawn(async move {
        let mut i: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            i += 1;
            let mut h = hub.lock().unwrap();
            h.queue_tick();
            if i % 30 == 0 {
                h.sweep();
            }
        }
    });
}

async fn cards_handler(State(s): State<AppState>) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=3600")], s.cards_json.as_str().to_owned()).into_response()
}

fn fnv(s: &str) -> u64 {
    rift_shared::rng::seed_from_str(s)
}

async fn img_handler(State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(u) = q.get("u") else { return (StatusCode::BAD_REQUEST, "missing u").into_response() };
    let splash = u.strip_prefix(SPLASH_PREFIX).is_some_and(|f| f.len() <= 40 && f.ends_with("_0.jpg") && f[..f.len() - 6].bytes().all(|b| b.is_ascii_alphanumeric()))
        || u.strip_prefix(WIKI_THUMB_PREFIX).is_some_and(wiki_splash_ok);
    if !(splash || u.starts_with(IMG_HOST)) || u.contains("..") || u.len() > 400 {
        return (StatusCode::FORBIDDEN, "host not allowed").into_response();
    }
    let w: u32 = q.get("w").and_then(|w| w.parse().ok()).unwrap_or(400).clamp(64, 1024);
    let (key, mime) = if splash { (format!("{:016x}.jpg", fnv(u)), "image/jpeg") } else { (format!("{:016x}_{w}.webp", fnv(u)), "image/webp") };
    let path = s.img_cache.join(&key);
    let headers = [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "public, max-age=604800, immutable")];
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return (headers, bytes).into_response();
    }
    let sep = if u.contains('?') { '&' } else { '?' };
    let url = if splash { u.clone() } else { format!("{u}{sep}w={w}&fm=webp") };
    match s.http.get(&url).send().await {
        Ok(r) if r.status().is_success() => match r.bytes().await {
            Ok(b) => {
                let tmp = path.with_extension("tmp");
                if tokio::fs::write(&tmp, &b).await.is_ok() {
                    let _ = tokio::fs::rename(&tmp, &path).await;
                }
                (headers, b.to_vec()).into_response()
            }
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        },
        Ok(r) => (StatusCode::BAD_GATEWAY, format!("upstream {}", r.status())).into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(s): State<AppState>, headers: HeaderMap) -> Response {
    // Only this site's pages may open the game socket.
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    ws.max_message_size(MAX_MSG_BYTES).on_upgrade(move |socket| client_loop(socket, s))
}

/// No Origin header (non-browser clients) is allowed; otherwise it must match Host.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin);
    !host.is_empty() && origin_host.eq_ignore_ascii_case(host)
}

async fn client_loop(socket: WebSocket, s: AppState) {
    let (mut sink, mut stream) = socket.split();
    // First message must be Hello.
    let first = tokio::time::timeout(Duration::from_secs(15), stream.next()).await;
    let (token, name) = match first {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<ClientMsg>(&t) {
            Ok(ClientMsg::Hello { token, name }) => (token, name),
            _ => return,
        },
        _ => return,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ServerMsg>();
    let (id, conn) = s.hub.lock().unwrap().hello(token.as_deref(), &name, tx);
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&msg) else { continue };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(t) => match serde_json::from_str::<ClientMsg>(&t) {
                Ok(m) => s.hub.lock().unwrap().handle(id, m),
                Err(e) => tracing::debug!("bad message from {id}: {e}"),
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    s.hub.lock().unwrap().disconnect(id, conn);
    writer.abort();
}

#[cfg(test)]
mod splash_tests {
    use super::wiki_splash_ok;

    #[test]
    fn wiki_splash_allow_list() {
        assert!(wiki_splash_ok("Jinx_OriginalSkin_HD.jpg/1920px-Jinx_OriginalSkin_HD.jpg"));
        assert!(wiki_splash_ok("Kai'Sa_OriginalSkin_HD.jpg/2560px-Kai'Sa_OriginalSkin_HD.jpg"));
        assert!(!wiki_splash_ok("Jinx_OriginalSkin_HD.jpg/4000px-Jinx_OriginalSkin_HD.jpg"));
        assert!(!wiki_splash_ok("Jinx_OriginalSkin_HD.jpg/1920px-Ahri_OriginalSkin_HD.jpg"));
        assert!(!wiki_splash_ok("Jinx.png/1920px-Jinx.png"));
        assert!(!wiki_splash_ok("a/b_OriginalSkin_HD.jpg/1920px-a/b_OriginalSkin_HD.jpg"));
    }
}
