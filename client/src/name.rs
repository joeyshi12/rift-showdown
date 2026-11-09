//! Player names. There are no accounts: before joining a lobby (creating,
//! joining or watching a room, or searching for a match) the player types the
//! name others will see. It is remembered in this browser and pre-filled.

use crate::app::App;
use crate::dom::{self, by_id, esc, input_value, set_html};
use crate::icons::icon;
use crate::net;
use rift_shared::protocol::{clean_name, ClientMsg, MAX_NAME_LEN};

const NAME_KEY: &str = "rift.name";

#[derive(Default)]
pub struct NameUi {
    /// What to send once the player confirms their name.
    pub pending: Option<ClientMsg>,
    /// Another tab took over this session; this one stopped.
    pub replaced: bool,
}

/// The saved name, or "Player" on a first visit.
pub fn saved() -> String {
    dom::local_get(NAME_KEY).map(|n| clean_name(&n)).unwrap_or_else(|| "Player".into())
}

/// Ask for a name, then send `msg` (CreateRoom, JoinRoom or Queue).
pub fn ask(app: &mut App, msg: ClientMsg) {
    app.names.pending = Some(msg);
    set_html("name-dlg", "");
    render_dialog(app);
    if let Some(el) = by_id("nm-name").and_then(|e| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlInputElement>(e).ok()) {
        let _ = el.focus();
        el.select();
    }
}

/// Returns true when the action was handled here.
pub fn handle(app: &mut App, act: &str) -> bool {
    match act {
        "name-go" => {
            let raw = input_value("nm-name");
            if raw.trim().is_empty() {
                set_html("nm-err", "Type a name to continue.");
                return true;
            }
            let name = clean_name(&raw);
            dom::local_set(NAME_KEY, &name);
            if name != app.name {
                app.name = name.clone();
                net::send(&ClientMsg::SetName { name });
            }
            if let Some(m) = app.names.pending.take() {
                app.joining = true;
                net::send(&m);
            }
            render_dialog(app);
        }
        "name-close" => {
            app.names.pending = None;
            crate::route::release(app);
            render_dialog(app);
        }
        "resume-here" => {
            app.names.replaced = false;
            net::reconnect();
            app.connected = false;
            app.render();
        }
        _ => return false,
    }
    true
}

fn dialog_html(app: &App) -> String {
    if app.names.replaced {
        return "<div class=\"modal-bg dark\"><div class=\"dialog au\" data-act=\"noop\" role=\"dialog\" aria-modal=\"true\"><h2 class=\"au-t\">Open in another tab</h2><p class=\"au-reason\">Rift Showdown is open in another tab, so this one stopped.</p><button class=\"btn primary au-go\" data-act=\"resume-here\">Use here</button></div></div>".into();
    }
    let Some(m) = &app.names.pending else { return String::new() };
    let (title, go) = match m {
        ClientMsg::CreateRoom { .. } => ("Create a room".to_string(), "Create room"),
        ClientMsg::JoinRoom { code, spectate: true } => (format!("Watch room {}", code.to_uppercase()), "Watch"),
        ClientMsg::JoinRoom { code, .. } => (format!("Join room {}", code.to_uppercase()), "Join"),
        ClientMsg::Queue { .. } => ("Find a match".to_string(), "Search"),
        _ => ("Your name".to_string(), "Continue"),
    };
    let value = if dom::local_get(NAME_KEY).is_some() { esc(&app.name) } else { String::new() };
    format!(
        r#"<div class="modal-bg au-bg" data-act="name-close"><div class="dialog au" data-act="noop" role="dialog" aria-modal="true" aria-labelledby="nm-t"><button class="au-x" data-act="name-close" aria-label="Close">{x}</button><h2 class="au-t" id="nm-t">{title}</h2>
<label class="fld"><span class="fl">Your name<em>Shown to other players</em></span><input id="nm-name" autocomplete="nickname" maxlength="{max}" value="{value}" placeholder="Player" data-enter="name-go"></label>
<div id="nm-err" class="au-err" role="alert"></div>
<button class="btn primary au-go" id="nm-go" data-act="name-go">{go}</button></div></div>"#,
        x = icon("close"),
        title = esc(&title),
        max = MAX_NAME_LEN,
    )
}

/// The dialog has its own container, so re-rendering a screen never clears
/// what the player is typing.
pub fn render_dialog(app: &App) {
    let html = dialog_html(app);
    let open = by_id("name-dlg").map(|e| !e.inner_html().is_empty()).unwrap_or(false);
    // Keep the open dialog (and its input) as is unless it should close or change.
    if open && !html.is_empty() && !app.names.replaced {
        return;
    }
    set_html("name-dlg", &html);
}
