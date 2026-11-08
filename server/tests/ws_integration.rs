//! End-to-end server test over real WebSockets: two clients create/join a room,
//! load decks, ready up, start a match and exchange game actions and chat.

use futures_util::{SinkExt, StreamExt};
use rift_server::auth::{AuthConfig, OAuthProvider};
use rift_server::mail::{Mail, Mailer};
use rift_server::{load_state, router, Config};

type Outbox = std::sync::Arc<std::sync::Mutex<Vec<Mail>>>;
use rift_shared::cards::CardDb;
use rift_shared::decklist::starter_deck;
use rift_shared::game::{Action, Stage};
use rift_shared::protocol::*;
use rift_shared::session::MatchAction;
use std::path::PathBuf;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, m: &ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(m).unwrap().into())).await.unwrap();
}

async fn until<T>(ws: &mut Ws, mut f: impl FnMut(&ServerMsg) -> Option<T>) -> T {
    let fut = async {
        loop {
            let msg = ws.next().await.expect("socket closed").unwrap();
            if let Message::Text(t) = msg {
                let m: ServerMsg = serde_json::from_str(&t).unwrap();
                if let Some(v) = f(&m) {
                    return v;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), fut).await.expect("timed out waiting for message")
}

async fn start(tag: &str, auth: impl FnOnce(String) -> AuthConfig) -> (std::net::SocketAddr, PathBuf) {
    let (addr, tmp, _) = start_with_mail(tag, auth).await;
    (addr, tmp)
}

async fn start_with_mail(tag: &str, auth: impl FnOnce(String) -> AuthConfig) -> (std::net::SocketAddr, PathBuf, Outbox) {
    let outbox: Outbox = Default::default();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let tmp = std::env::temp_dir().join(format!("rift-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(root.join("data/cards.json"), tmp.join("cards.json")).unwrap();
    std::fs::write(tmp.join("index.html"), "<!doctype html><title>Rift</title>").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = Config { data_dir: tmp.clone(), dist_dir: tmp.clone(), accounts_db: None, img_cache: None, auth: auth(format!("http://{addr}")), mailer: Mailer::Memory(outbox.clone()) };
    let state = load_state(&cfg).unwrap();
    let app = router(state, tmp.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, tmp, outbox)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

/// POST JSON; returns (status, body, session cookie if set).
async fn post(base: &str, path: &str, body: serde_json::Value, cookie: Option<&str>) -> (u16, serde_json::Value, Option<String>) {
    let mut r = client().post(format!("{base}{path}")).header("content-type", "application/json").body(body.to_string());
    if let Some(c) = cookie {
        r = r.header("cookie", c);
    }
    let r = r.send().await.unwrap();
    let status = r.status().as_u16();
    let set = session_from(r.headers());
    let v = serde_json::from_str(&r.text().await.unwrap()).unwrap_or(serde_json::Value::Null);
    (status, v, set)
}

fn session_from(h: &reqwest::header::HeaderMap) -> Option<String> {
    h.get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).find(|v| v.starts_with("rift_session=") && !v.starts_with("rift_session=;")).map(|v| v.split(';').next().unwrap().to_string())
}

async fn ws(addr: std::net::SocketAddr, cookie: Option<&str>) -> Ws {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    if let Some(c) = cookie {
        req.headers_mut().insert("cookie", c.parse().unwrap());
    }
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    send(&mut w, &ClientMsg::Hello { token: None }).await;
    w
}

async fn welcome(w: &mut Ws) -> (String, bool) {
    until(w, |m| if let ServerMsg::Welcome { name, signed_in, .. } = m { Some((name.clone(), *signed_in)) } else { None }).await
}

#[tokio::test]
async fn accounts_over_http() {
    let (addr, tmp) = start("acct", |_| AuthConfig::default()).await;
    let base = format!("http://{addr}");
    let me: serde_json::Value = serde_json::from_str(&client().get(format!("{base}/api/me")).send().await.unwrap().text().await.unwrap()).unwrap();
    assert!(me["user"].is_null());
    assert_eq!(me["providers"].as_array().unwrap().len(), 0);

    let pw = "a long enough password";
    let (st, v, _) = post(&base, "/api/sign-up", serde_json::json!({"name":"Alice","email":"alice@example.com","password":"short"}), None).await;
    assert_eq!(st, 400, "{v}");
    assert!(v["error"].as_str().unwrap().contains("12"));
    let (st, v, cookie) = post(&base, "/api/sign-up", serde_json::json!({"name":"Alice","email":"alice@example.com","password":pw}), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["user"]["name"], "Alice");
    assert!(v["user"].get("pw_hash").is_none());
    let alice = cookie.expect("session cookie");
    let (st, _, _) = post(&base, "/api/sign-up", serde_json::json!({"name":"alice","email":"other@example.com","password":pw}), None).await;
    assert_eq!(st, 409);

    // Cookie is HttpOnly and SameSite=Lax.
    let r = client().post(format!("{base}/api/sign-in")).header("content-type", "application/json").body(serde_json::json!({"email":"ALICE@example.com","password":pw}).to_string()).send().await.unwrap();
    let raw = r.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    assert!(raw.contains("HttpOnly") && raw.contains("SameSite=Lax"), "{raw}");
    let (st, _, _) = post(&base, "/api/sign-in", serde_json::json!({"email":"alice@example.com","password":"wrong password here"}), None).await;
    assert_eq!(st, 401);

    // Cross-origin and non-JSON POSTs are refused.
    let r = client().post(format!("{base}/api/account")).header("content-type", "application/json").header("origin", "https://evil.example").header("cookie", &alice).body(r#"{"name":"Hacked"}"#).send().await.unwrap();
    assert_eq!(r.status(), 403);
    let r = client().post(format!("{base}/api/sign-out")).header("content-type", "text/plain").header("cookie", &alice).send().await.unwrap();
    assert_eq!(r.status(), 403);
    // Cross-origin WebSocket handshakes are refused.
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
        req.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
        req.headers_mut().insert("cookie", alice.parse().unwrap());
        assert!(tokio_tungstenite::connect_async(req).await.is_err());
    }

    // Two accounts get two separate identities on the socket.
    let (_, _, bob) = post(&base, "/api/sign-up", serde_json::json!({"name":"Bob","email":"bob@example.com","password":pw}), None).await;
    let bob = bob.unwrap();
    let mut wa = ws(addr, Some(&alice)).await;
    let mut wb = ws(addr, Some(&bob)).await;
    assert_eq!(welcome(&mut wa).await, ("Alice".into(), true));
    assert_eq!(welcome(&mut wb).await, ("Bob".into(), true));
    send(&mut wa, &ClientMsg::CreateRoom { settings: RoomSettings::default() }).await;
    let code = until(&mut wa, |m| if let ServerMsg::Room { room } = m { Some(room.code.clone()) } else { None }).await;
    send(&mut wb, &ClientMsg::JoinRoom { code: code.clone(), spectate: false }).await;
    until(&mut wa, |m| if let ServerMsg::Room { room } = m { (room.players.len() == 2).then_some(()) } else { None }).await;

    // Renaming Alice changes only Alice, and the room sees it.
    let (st, v, _) = post(&base, "/api/account", serde_json::json!({"name":"Alicia"}), Some(&alice)).await;
    assert_eq!(st, 200, "{v}");
    let names = until(&mut wb, |m| if let ServerMsg::Room { room } = m { let n: Vec<String> = room.players.iter().map(|p| p.name.clone()).collect(); (n[0] == "Alicia").then_some(n) } else { None }).await;
    assert_eq!(names, vec!["Alicia", "Bob"]);
    let (st, _, _) = post(&base, "/api/account", serde_json::json!({"name":"Bob"}), Some(&alice)).await;
    assert_eq!(st, 409);

    // A guest socket can watch but not play.
    let mut wg = ws(addr, None).await;
    assert_eq!(welcome(&mut wg).await, ("Guest".into(), false));
    send(&mut wg, &ClientMsg::CreateRoom { settings: RoomSettings::default() }).await;
    until(&mut wg, |m| matches!(m, ServerMsg::SignInRequired { .. }).then_some(())).await;
    send(&mut wg, &ClientMsg::JoinRoom { code, spectate: true }).await;
    until(&mut wg, |m| if let ServerMsg::Room { room } = m { (room.you == Role::Spectator).then_some(()) } else { None }).await;

    // Opening the same account again retires the first socket.
    let mut wa2 = ws(addr, Some(&alice)).await;
    welcome(&mut wa2).await;
    until(&mut wa, |m| matches!(m, ServerMsg::Replaced).then_some(())).await;

    // Sign out ends the session.
    let (st, _, _) = post(&base, "/api/sign-out", serde_json::json!({}), Some(&alice)).await;
    assert_eq!(st, 200);
    let me: serde_json::Value = serde_json::from_str(&client().get(format!("{base}/api/me")).header("cookie", &alice).send().await.unwrap().text().await.unwrap()).unwrap();
    assert!(me["user"].is_null());
    let mut wa3 = ws(addr, Some(&alice)).await;
    assert!(!welcome(&mut wa3).await.1);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn password_reset_by_email() {
    let (addr, tmp, outbox) = start_with_mail("reset", |public| AuthConfig { public_url: Some(public), ..Default::default() }).await;
    let base = format!("http://{addr}");
    let pw = "the original password";
    let (_, _, old) = post(&base, "/api/sign-up", serde_json::json!({"name":"Rita","email":"rita@example.com","password":pw}), None).await;
    let old = old.unwrap();
    let mut sock = ws(addr, Some(&old)).await;
    welcome(&mut sock).await;

    // Unknown and known addresses get the same answer; only the known one gets mail.
    let (st, v, _) = post(&base, "/api/forgot-password", serde_json::json!({"email":"nobody@example.com"}), None).await;
    assert_eq!((st, v.clone()), (200, serde_json::json!({"ok": true})));
    let (st, v2, _) = post(&base, "/api/forgot-password", serde_json::json!({"email":"RITA@example.com"}), None).await;
    assert_eq!((st, v2), (200, v));
    let (st, _, _) = post(&base, "/api/forgot-password", serde_json::json!({"email":"nope"}), None).await;
    assert_eq!(st, 400);
    let mail = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(m) = outbox.lock().unwrap().first().cloned() {
                return m;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reset email sent");
    assert_eq!(outbox.lock().unwrap().len(), 1);
    assert_eq!(mail.to, "rita@example.com");
    assert!(mail.body.contains("Hi Rita"));
    let link = mail.body.lines().find(|l| l.starts_with(&format!("{base}/?reset="))).expect("link in email").to_string();
    let token = link.rsplit('=').next().unwrap().to_string();

    let (st, v, _) = post(&base, "/api/reset-password", serde_json::json!({"token":token,"password":"short"}), None).await;
    assert_eq!(st, 400, "{v}");
    let (st, v, fresh) = post(&base, "/api/reset-password", serde_json::json!({"token":token,"password":"a brand new password"}), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["user"]["name"], "Rita");
    let fresh = fresh.expect("signed in after reset");
    // The open socket of the old session is told to sign out.
    until(&mut sock, |m| matches!(m, ServerMsg::SignedOut).then_some(())).await;
    let me = |c: String| async move { serde_json::from_str::<serde_json::Value>(&client().get(format!("http://{addr}/api/me")).header("cookie", c).send().await.unwrap().text().await.unwrap()).unwrap() };
    assert!(me(old).await["user"].is_null(), "old session revoked");
    assert_eq!(me(fresh).await["user"]["name"], "Rita");
    let (st, _, _) = post(&base, "/api/sign-in", serde_json::json!({"email":"rita@example.com","password":pw}), None).await;
    assert_eq!(st, 401);
    let (st, _, _) = post(&base, "/api/sign-in", serde_json::json!({"email":"rita@example.com","password":"a brand new password"}), None).await;
    assert_eq!(st, 200);
    // The link is single use.
    let (st, v, _) = post(&base, "/api/reset-password", serde_json::json!({"token":token,"password":"yet another password"}), None).await;
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("expired"));
    // At most 3 reset emails per address per 15 minutes (one already used above).
    for want in [200, 200, 429] {
        let (st, _, _) = post(&base, "/api/forgot-password", serde_json::json!({"email":"rita@example.com"}), None).await;
        assert_eq!(st, want);
    }
    // Responses carry a referrer policy so the token never leaks via Referer.
    let r = client().get(format!("{base}/?reset=x")).send().await.unwrap();
    assert_eq!(r.headers().get("referrer-policy").unwrap(), "same-origin");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A fake OAuth provider: authorize redirects straight back with a code;
/// token and userinfo endpoints check what the server sends.
async fn mock_provider() -> String {
    use axum::extract::Query;
    use axum::routing::{get, post};
    use std::collections::HashMap;
    async fn authorize(Query(q): Query<HashMap<String, String>>) -> axum::response::Response {
        use axum::response::IntoResponse;
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["client_id"], "cid");
        let to = format!("{}?code=good-code&state={}", q["redirect_uri"], q["state"]);
        (axum::http::StatusCode::FOUND, [("location", to)]).into_response()
    }
    async fn token(body: String) -> axum::response::Response {
        use axum::response::IntoResponse;
        let f: HashMap<String, String> = serde_urlencoded::from_str(&body).unwrap();
        if f.get("code").map(|c| c.as_str()) != Some("good-code") || f["client_secret"] != "secret" || f.get("code_verifier").map(|v| v.len()) != Some(64) {
            return (axum::http::StatusCode::BAD_REQUEST, "bad").into_response();
        }
        axum::Json(serde_json::json!({"access_token":"tok123","token_type":"Bearer"})).into_response()
    }
    async fn userinfo(h: axum::http::HeaderMap) -> axum::response::Response {
        use axum::response::IntoResponse;
        if h.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer tok123") {
            return axum::http::StatusCode::UNAUTHORIZED.into_response();
        }
        axum::Json(serde_json::json!({"sub":"g-42","name":"Gwen","email":"gwen@example.com","email_verified":true})).into_response()
    }
    let app = axum::Router::new().route("/authorize", get(authorize)).route("/token", post(token)).route("/userinfo", get(userinfo));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    base
}

#[tokio::test]
async fn oauth_sign_in_with_mock_provider() {
    let idp = mock_provider().await;
    let idp2 = idp.clone();
    let (addr, tmp) = start("oauth", move |public| AuthConfig {
        public_url: Some(public),
        secure_cookies: false,
        providers: vec![OAuthProvider {
            id: "google".into(),
            label: "Google".into(),
            client_id: "cid".into(),
            client_secret: "secret".into(),
            auth_url: format!("{idp2}/authorize"),
            token_url: format!("{idp2}/token"),
            userinfo_url: format!("{idp2}/userinfo"),
            scope: "openid email profile".into(),
        }],
    })
    .await;
    let base = format!("http://{addr}");
    let me: serde_json::Value = serde_json::from_str(&client().get(format!("{base}/api/me")).send().await.unwrap().text().await.unwrap()).unwrap();
    assert_eq!(me["providers"][0]["id"], "google");

    // start -> provider authorize -> our callback
    let r = client().get(format!("{base}/auth/google/start?redirect=%2F%3Froom%3DABCDEF")).send().await.unwrap();
    assert_eq!(r.status(), 302);
    let state_cookie = r.headers().get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let to = r.headers().get("location").unwrap().to_str().unwrap().to_string();
    assert!(to.starts_with(&idp));
    let r = client().get(&to).send().await.unwrap();
    let callback = r.headers().get("location").unwrap().to_str().unwrap().to_string();
    assert!(callback.starts_with(&format!("{base}/auth/google/callback")));

    // Without the state cookie (login CSRF) the callback refuses.
    let r = client().get(&callback).send().await.unwrap();
    let loc = r.headers().get("location").unwrap().to_str().unwrap().to_string();
    assert!(loc.contains("auth_error"), "{loc}");
    assert!(session_from(r.headers()).is_none());

    // Proper round trip.
    let r = client().get(format!("{base}/auth/google/start?redirect=%2F%3Froom%3DABCDEF")).send().await.unwrap();
    let state_cookie2 = r.headers().get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    assert_ne!(state_cookie, state_cookie2);
    let to = r.headers().get("location").unwrap().to_str().unwrap().to_string();
    let callback = client().get(&to).send().await.unwrap().headers().get("location").unwrap().to_str().unwrap().to_string();
    let r = client().get(&callback).header("cookie", &state_cookie2).send().await.unwrap();
    assert_eq!(r.status(), 302);
    assert_eq!(r.headers().get("location").unwrap(), "/?room=ABCDEF");
    let session = session_from(r.headers()).expect("signed in");
    let me: serde_json::Value = serde_json::from_str(&client().get(format!("{base}/api/me")).header("cookie", &session).send().await.unwrap().text().await.unwrap()).unwrap();
    assert_eq!(me["user"]["name"], "Gwen");
    assert_eq!(me["user"]["providers"][0], "google");
    assert_eq!(me["user"]["has_password"], false);
    // The state is single use.
    let r = client().get(&callback).header("cookie", &state_cookie2).send().await.unwrap();
    assert!(r.headers().get("location").unwrap().to_str().unwrap().contains("auth_error"));
    // Unknown provider
    assert_eq!(client().get(format!("{base}/auth/discord/start")).send().await.unwrap().status(), 404);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn two_players_over_websocket() {
    let (addr, tmp) = start("ws", |_| AuthConfig::default()).await;

    // HTTP routes
    let base = format!("http://{addr}");
    let health = reqwest::get(format!("{base}/healthz")).await.unwrap().text().await.unwrap();
    assert_eq!(health, "ok");
    let cards = reqwest::get(format!("{base}/cards.json")).await.unwrap().text().await.unwrap();
    assert!(cards.len() > 100_000);
    let spa = reqwest::get(format!("{base}/some/deep/link")).await.unwrap().text().await.unwrap();
    assert!(spa.contains("<title>Rift</title>"));
    let forbidden = reqwest::get(format!("{base}/img?u=https://evil.example/x.png")).await.unwrap().status();
    assert_eq!(forbidden, 403);

    let db = CardDb::from_json(&cards).unwrap();
    let pw = "a long enough password";
    let (_, _, ca) = post(&base, "/api/sign-up", serde_json::json!({"name":"Alice","email":"a@example.com","password":pw}), None).await;
    let (_, _, cb) = post(&base, "/api/sign-up", serde_json::json!({"name":"Bob","email":"b@example.com","password":pw}), None).await;
    let mut a = ws(addr, ca.as_deref()).await;
    let mut b = ws(addr, cb.as_deref()).await;
    welcome(&mut a).await;
    welcome(&mut b).await;

    send(&mut a, &ClientMsg::CreateRoom { settings: RoomSettings::default() }).await;
    let code = until(&mut a, |m| if let ServerMsg::Room { room } = m { Some(room.code.clone()) } else { None }).await;
    send(&mut b, &ClientMsg::JoinRoom { code: code.clone(), spectate: false }).await;
    until(&mut b, |m| if let ServerMsg::Room { room } = m { (room.players.len() == 2).then_some(()) } else { None }).await;

    send(&mut a, &ClientMsg::SetDeck { deck: starter_deck(&db, "SFD-181").unwrap(), sealed: None }).await;
    send(&mut b, &ClientMsg::SetDeck { deck: starter_deck(&db, "UNL-193").unwrap(), sealed: None }).await;
    send(&mut a, &ClientMsg::SetReady { ready: true }).await;
    send(&mut b, &ClientMsg::SetReady { ready: true }).await;
    let view = until(&mut a, |m| if let ServerMsg::Match { view } = m { Some(view.clone()) } else { None }).await;
    assert_eq!(view.game.stage, Stage::ChooseBattlefield);
    assert_eq!(view.viewer, Some(0));

    let bf = view.game.seats[0].bf_options[0].clone();
    send(&mut a, &ClientMsg::Act { action: MatchAction::Game(Action::ChooseBattlefield { code: bf }) }).await;
    until(&mut b, |m| if let ServerMsg::Match { view } = m { view.game.seats[0].chosen_bf.is_some().then_some(()) } else { None }).await;

    send(&mut b, &ClientMsg::Chat { text: "good luck".into() }).await;
    until(&mut a, |m| if let ServerMsg::Chat { line } = m { (line.text == "good luck").then_some(()) } else { None }).await;

    // Illegal action yields an error only to the sender.
    send(&mut b, &ClientMsg::Act { action: MatchAction::Game(Action::EndTurn) }).await;
    until(&mut b, |m| matches!(m, ServerMsg::Error { .. }).then_some(())).await;

    send(&mut a, &ClientMsg::Act { action: MatchAction::Game(Action::Concede) }).await;
    let w = until(&mut b, |m| if let ServerMsg::Match { view } = m { view.game.winner } else { None }).await;
    assert_eq!(w, 1);
    send(&mut a, &ClientMsg::ReturnToLobby).await;
    until(&mut b, |m| if let ServerMsg::Room { room } = m { (!room.in_game).then_some(()) } else { None }).await;
    let _ = std::fs::remove_dir_all(&tmp);
}
