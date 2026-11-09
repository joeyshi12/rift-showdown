//! URLs for screens, so a reload or the back button keeps your place.
//!
//!   /                 Duel (home)
//!   /solo  /sealed  /decks  /rooms
//!   /decks/<id>       deck editor (`/decks/new` for an unsaved deck)
//!   /room/<CODE>      room lobby or its match (`?spectate=1` to watch)
//!   /practice         a solo or goldfish game (memory only: a reload lands on /solo)
//!
//! The session token is kept per tab, so reloading a room page resumes the same
//! seat. Old `/?room=CODE` invite links still work.

use crate::app::{App, Screen, Tab};
use crate::dom;
use rift_shared::protocol::ClientMsg;
use wasm_bindgen::JsValue;

const SESSION_KEY: &str = "rift.session";

fn session_storage() -> Option<web_sys::Storage> {
    dom::window().session_storage().ok().flatten()
}

pub fn saved_token() -> Option<String> {
    session_storage().and_then(|s| s.get_item(SESSION_KEY).ok().flatten())
}

pub fn save_token(t: &str) {
    if let Some(s) = session_storage() {
        let _ = s.set_item(SESSION_KEY, t);
    }
}

fn pathname() -> String {
    dom::window().location().pathname().unwrap_or_else(|_| "/".into())
}

/// The room code in a `/room/<CODE>` path or an old `?room=CODE` link.
fn room_in(path: &str) -> Option<String> {
    let code = path.strip_prefix("/room/").map(|c| c.trim_end_matches('/').to_string()).or_else(|| dom::query_param("room"))?;
    let code = code.to_uppercase();
    (code.len() == 6 && code.chars().all(|c| c.is_ascii_alphanumeric())).then_some(code)
}

fn tab_of(path: &str) -> Option<Tab> {
    Some(match path.trim_end_matches('/') {
        "" => Tab::Duel,
        "/solo" | "/practice" => Tab::Solo,
        "/sealed" => Tab::Sealed,
        "/decks" => Tab::Decks,
        "/rooms" => Tab::Rooms,
        _ => return None,
    })
}

/// The path for what is on screen now.
pub fn path_for(app: &App) -> String {
    match app.screen {
        Screen::Home(Tab::Duel) => "/".into(),
        Screen::Home(Tab::Solo) => "/solo".into(),
        Screen::Home(Tab::Sealed) => "/sealed".into(),
        Screen::Home(Tab::Decks) => "/decks".into(),
        Screen::Home(Tab::Rooms) => "/rooms".into(),
        Screen::Editor => match (&app.editor.deck_id, &app.editor.sealed) {
            (Some(id), _) => format!("/decks/{}", dom::encode_uri(id)),
            (None, Some(_)) => "/sealed".into(),
            (None, None) => "/decks/new".into(),
        },
        Screen::Game if app.game.as_ref().is_some_and(|g| g.is_local()) => "/practice".into(),
        Screen::Lobby | Screen::Game => match &app.room {
            Some(r) => format!("/room/{}", r.code),
            None => "/".into(),
        },
    }
}

/// Called after every render: record a new screen as a history entry.
pub fn sync(app: &App) {
    if app.route_hold.is_some() {
        return;
    }
    let want = path_for(app);
    let have = pathname();
    if want != have || dom::query_param("room").is_some() {
        let _ = dom::window().history().map(|h| h.push_state_with_url(&JsValue::NULL, "", Some(&want)));
    }
}

/// First screen from the address bar, before the socket connects. A room page
/// waits for the server (see `on_welcome`) and keeps its URL meanwhile.
pub fn boot(app: &mut App) {
    let path = pathname();
    if let Some(code) = room_in(&path) {
        app.route_hold = Some(code);
        return;
    }
    if let Some(id) = path.strip_prefix("/decks/") {
        let id = js_sys::decode_uri_component(id).ok().and_then(|s| s.as_string()).unwrap_or_default();
        if id == "new" {
            crate::ui_menu::open_editor(app, None, None);
            return;
        }
        if let Some(d) = app.deck_by_id(&id).cloned() {
            crate::ui_menu::open_editor(app, Some(d), None);
            return;
        }
    }
    app.screen = Screen::Home(tab_of(&path).unwrap_or(Tab::Duel));
    // Unknown paths and /practice (games are not saved) settle on a real page.
    let _ = dom::window().history().map(|h| h.replace_state_with_url(&JsValue::NULL, "", Some(&path_for(app))));
}

/// The server answered Hello. `resumed` means it knew our token, so if we were
/// in the room it is sending it right now; otherwise ask to join.
pub fn on_welcome(app: &mut App, resumed: bool) {
    let Some(code) = app.route_hold.clone() else { return };
    let spectate = dom::query_param("spectate").is_some();
    if resumed {
        // The Room message follows Welcome at once if we still hold a seat.
        dom::set_timeout(800, move || {
            crate::app::with_app(|a| {
                if a.route_hold.as_deref() == Some(code.as_str()) && a.names.pending.is_none() {
                    crate::name::ask(a, ClientMsg::JoinRoom { code, spectate });
                }
            });
        });
    } else {
        crate::name::ask(app, ClientMsg::JoinRoom { code, spectate });
    }
}

/// A Room message arrived: the held room page can show it now. If the server
/// resumed our seat while the join prompt was open, the prompt is moot.
pub fn on_room(app: &mut App, code: &str) {
    if app.route_hold.as_deref() == Some(code) {
        app.route_hold = None;
    }
    if matches!(&app.names.pending, Some(ClientMsg::JoinRoom { code: c, .. }) if c.eq_ignore_ascii_case(code)) {
        app.names.pending = None;
        dom::set_html("name-dlg", "");
    }
}

/// Stop holding a room URL (prompt cancelled, join refused): show the real page.
pub fn release(app: &mut App) {
    if app.route_hold.take().is_some() {
        let _ = dom::window().history().map(|h| h.replace_state_with_url(&JsValue::NULL, "", Some(&path_for(app))));
    }
}

/// Back and forward buttons.
pub fn on_popstate(app: &mut App) {
    let path = pathname();
    if let Some(code) = room_in(&path) {
        match &app.room {
            Some(r) if r.code == code => {
                let s = if r.in_game && app.game.as_ref().is_some_and(|g| !g.is_local()) { Screen::Game } else { Screen::Lobby };
                app.go(s);
            }
            _ => {
                // Keep the room URL while we find out (see `on_welcome`).
                app.route_hold = Some(code.clone());
                app.go(Screen::Home(Tab::Duel));
                if app.connected && app.my_id != 0 {
                    crate::name::ask(app, ClientMsg::JoinRoom { code, spectate: dom::query_param("spectate").is_some() });
                }
            }
        }
        return;
    }
    if path == "/practice" && app.game.as_ref().is_some_and(|g| g.is_local()) {
        app.go(Screen::Game);
        return;
    }
    if let Some(id) = path.strip_prefix("/decks/") {
        let id = js_sys::decode_uri_component(id).ok().and_then(|s| s.as_string()).unwrap_or_default();
        if app.screen == Screen::Editor && app.editor.deck_id.as_deref() == Some(id.as_str()) {
            return;
        }
        match app.deck_by_id(&id).cloned() {
            Some(d) => crate::ui_menu::open_editor(app, Some(d), None),
            None if id == "new" => crate::ui_menu::open_editor(app, None, None),
            None => app.go(Screen::Home(Tab::Decks)),
        }
        return;
    }
    app.go(Screen::Home(tab_of(&path).unwrap_or(Tab::Duel)));
}
