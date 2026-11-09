//! End-to-end server test over real WebSockets: two clients create/join a room,
//! load decks, ready up, start a match and exchange game actions and chat.

use futures_util::{SinkExt, StreamExt};
use rift_server::{load_state, router, Config};
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

async fn start(tag: &str) -> (std::net::SocketAddr, PathBuf) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let tmp = std::env::temp_dir().join(format!("rift-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(root.join("data/cards.json"), tmp.join("cards.json")).unwrap();
    std::fs::write(tmp.join("index.html"), "<!doctype html><title>Rift</title>").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = Config { data_dir: tmp.clone(), dist_dir: tmp.clone(), img_cache: None };
    let state = load_state(&cfg).unwrap();
    let app = router(state, tmp.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, tmp)
}

async fn ws(addr: std::net::SocketAddr, name: &str) -> Ws {
    let (mut w, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
    send(&mut w, &ClientMsg::Hello { token: None, name: name.into() }).await;
    w
}

async fn welcome(w: &mut Ws) -> String {
    until(w, |m| if let ServerMsg::Welcome { name, .. } = m { Some(name.clone()) } else { None }).await
}

#[tokio::test]
async fn cross_site_sockets_are_refused() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (addr, tmp) = start("origin").await;
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("origin", format!("http://{addr}").parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_ok());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn two_players_over_websocket() {
    let (addr, tmp) = start("ws").await;

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
    let mut a = ws(addr, "Alice").await;
    let mut b = ws(addr, "Bob").await;
    assert_eq!(welcome(&mut a).await, "Alice");
    assert_eq!(welcome(&mut b).await, "Bob");

    send(&mut a, &ClientMsg::CreateRoom { settings: RoomSettings::default() }).await;
    let code = until(&mut a, |m| if let ServerMsg::Room { room } = m { Some(room.code.clone()) } else { None }).await;
    send(&mut b, &ClientMsg::JoinRoom { code: code.clone(), spectate: false }).await;
    let names = until(&mut b, |m| if let ServerMsg::Room { room } = m { (room.players.len() == 2).then(|| room.players.iter().map(|p| p.name.clone()).collect::<Vec<_>>()) } else { None }).await;
    assert_eq!(names, ["Alice", "Bob"]);

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
