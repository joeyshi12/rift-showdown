//! Accounts: sign in / sign up dialog, account settings and the top-bar
//! account menu. Talks to the server's `/api/*` routes; the session itself is
//! an HttpOnly cookie, so the client never sees the token.

use crate::app::{with_app, App};
use crate::dom::{self, by_id, esc, input_value, set_html};
use crate::icons::icon;
use crate::net;
use serde::Deserialize;
use wasm_bindgen::JsCast;

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub email: Option<String>,
    #[serde(default)]
    pub has_password: bool,
    #[serde(default)]
    pub providers: Vec<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Provider {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthView {
    SignIn,
    SignUp,
    Forgot,
    /// Choosing a new password from an emailed link.
    Reset,
    Account,
}

#[derive(Default)]
pub struct AuthUi {
    pub user: Option<Account>,
    pub providers: Vec<Provider>,
    pub min_password: usize,
    /// `/api/me` answered (until then the top bar shows nothing).
    pub loaded: bool,
    pub view: Option<AuthView>,
    /// Why the dialog opened, e.g. "Sign in to play online."
    pub reason: Option<String>,
    pub error: Option<String>,
    pub busy: bool,
    pub menu_open: bool,
    /// Another tab took over this account's connection.
    pub replaced: bool,
    /// Token from a `/?reset=` link.
    pub reset_token: Option<String>,
    /// The forgot-password form was sent for this address.
    pub sent_to: Option<String>,
}

impl AuthUi {
    pub fn signed_in(&self) -> bool {
        self.user.is_some()
    }
}

// ------------------------------------------------------------------ HTTP

async fn api(method: &str, path: &str, body: Option<serde_json::Value>) -> Result<(u16, serde_json::Value), String> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    let headers = web_sys::Headers::new().map_err(|_| "headers")?;
    if let Some(b) = &body {
        let _ = headers.set("Content-Type", "application/json");
        init.set_body(&wasm_bindgen::JsValue::from_str(&b.to_string()));
    }
    init.set_headers(&headers);
    let req = web_sys::Request::new_with_str_and_init(path, &init).map_err(|_| "request")?;
    let resp = wasm_bindgen_futures::JsFuture::from(dom::window().fetch_with_request(&req)).await.map_err(|_| "Can't reach the server. Check your connection.".to_string())?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "bad response")?;
    let status = resp.status();
    let text = wasm_bindgen_futures::JsFuture::from(resp.text().map_err(|_| "no body")?).await.map_err(|_| "read failed")?;
    let v = serde_json::from_str(&text.as_string().unwrap_or_default()).unwrap_or(serde_json::Value::Null);
    Ok((status, v))
}

fn error_of(status: u16, v: &serde_json::Value) -> String {
    v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("Request failed ({status})."))
}

fn user_of(v: &serde_json::Value) -> Option<Account> {
    v.get("user").and_then(|u| serde_json::from_value(u.clone()).ok())
}

/// Load the current session and enabled providers, then connect the socket.
pub fn boot() {
    if let Some(msg) = dom::query_param("auth_error") {
        with_app(|a| {
            a.auth.view = Some(AuthView::SignIn);
            a.auth.error = Some(msg);
        });
        strip_query_param("auth_error");
    }
    if let Some(token) = dom::query_param("reset") {
        // Take the token out of the address bar and history right away.
        strip_query_param("reset");
        with_app(|a| {
            a.auth.reset_token = Some(token);
            a.auth.view = Some(AuthView::Reset);
        });
    }
    wasm_bindgen_futures::spawn_local(async {
        let r = api("GET", "/api/me", None).await;
        with_app(|a| {
            if let Ok((200, v)) = &r {
                a.auth.user = user_of(v);
                a.auth.providers = v.get("providers").and_then(|p| serde_json::from_value(p.clone()).ok()).unwrap_or_default();
                a.auth.min_password = v.get("min_password").and_then(|n| n.as_u64()).unwrap_or(12) as usize;
            }
            a.auth.loaded = true;
            if let Some(u) = &a.auth.user {
                a.name = u.name.clone();
            }
            a.dirty = true;
            a.render();
        });
        net::connect(crate::app::hello);
    });
}

fn strip_query_param(name: &str) {
    let loc = dom::window().location();
    let Ok(search) = loc.search() else { return };
    let Ok(p) = web_sys::UrlSearchParams::new_with_str(&search) else { return };
    p.delete(name);
    let q = p.to_string().as_string().unwrap_or_default();
    let url = format!("{}{}", loc.pathname().unwrap_or_else(|_| "/".into()), if q.is_empty() { String::new() } else { format!("?{q}") });
    let _ = dom::window().history().map(|h| h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url)));
}

/// Called from a click handler, which already holds the app, so the sync part takes it directly.
fn submit(app: &mut App, path: &'static str, body: serde_json::Value) {
    app.auth.busy = true;
    app.auth.error = None;
    render_dialog(app);
    wasm_bindgen_futures::spawn_local(async move {
        let r = api("POST", path, Some(body)).await;
        with_app(|a| {
            a.auth.busy = false;
            match r {
                Ok((200, _)) if path == "/api/forgot-password" => {
                    a.auth.sent_to = Some(input_value("fp-email").trim().to_string());
                }
                Ok((200, v)) => {
                    let was = a.auth.user.as_ref().map(|u| u.id);
                    a.auth.user = user_of(&v);
                    if let Some(u) = &a.auth.user {
                        a.name = u.name.clone();
                    }
                    let now = a.auth.user.as_ref().map(|u| u.id);
                    if path == "/api/account" {
                        a.toast("Player name saved.", false);
                    } else {
                        a.auth.reset_token = None;
                        a.auth.view = None;
                        a.auth.reason = None;
                    }
                    if was != now || path == "/api/reset-password" {
                        // The socket identity comes from the cookie, so reconnect.
                        a.room = None;
                        a.queue = None;
                        if a.game.as_ref().map(|g| !g.is_local()).unwrap_or(false) {
                            a.game = None;
                            a.screen = crate::app::Screen::Home(crate::app::Tab::Duel);
                        }
                        net::reconnect();
            a.connected = false;
                        if let Some(u) = &a.auth.user {
                            let msg = if path == "/api/reset-password" { format!("Password changed. Signed in as {}.", u.name) } else { format!("Signed in as {}.", u.name) };
                            a.toast(&msg, false);
                        }
                    }
                }
                Ok((status, v)) => a.auth.error = Some(error_of(status, &v)),
                Err(e) => a.auth.error = Some(e),
            }
            a.dirty = true;
            a.render();
        });
    });
}

/// The server ended this account's sessions (a password reset). If this
/// browser did the reset it already holds a new session; otherwise sign out.
pub fn on_signed_out(app: &mut App) {
    if app.auth.busy && app.auth.view == Some(AuthView::Reset) {
        return; // our own reset; its response reconnects
    }
    wasm_bindgen_futures::spawn_local(async {
        let r = api("GET", "/api/me", None).await;
        with_app(|a| {
            let user = match &r {
                Ok((200, v)) => user_of(v),
                _ => None,
            };
            if user.is_none() {
                a.auth.user = None;
                a.name = "Player".into();
                a.room = None;
                a.queue = None;
                if a.game.as_ref().map(|g| !g.is_local()).unwrap_or(false) {
                    a.game = None;
                    a.screen = crate::app::Screen::Home(crate::app::Tab::Duel);
                }
                a.toast("Your password was changed, so you were signed out. Sign in again.", true);
            } else {
                a.auth.user = user;
            }
            net::reconnect();
            a.connected = false;
            a.dirty = true;
            a.render();
        });
    });
}

fn sign_out() {
    wasm_bindgen_futures::spawn_local(async {
        let r = api("POST", "/api/sign-out", Some(serde_json::json!({}))).await;
        with_app(|a| {
            if let Err(e) = r {
                a.toast(&e, true);
                return;
            }
            a.auth.user = None;
            a.auth.view = None;
            a.auth.menu_open = false;
            a.name = "Player".into();
            a.room = None;
            a.queue = None;
            if a.game.as_ref().map(|g| !g.is_local()).unwrap_or(false) {
                a.game = None;
                a.screen = crate::app::Screen::Home(crate::app::Tab::Duel);
            }
            net::reconnect();
            a.connected = false;
            a.toast("Signed out.", false);
            a.dirty = true;
            a.render();
        });
    });
}

// ------------------------------------------------------------------ actions

/// Open the sign-in dialog when an online action needs an account.
/// Returns true if the user is signed in and may continue.
pub fn require(app: &mut App, reason: &str) -> bool {
    if app.auth.signed_in() {
        return true;
    }
    open(app, AuthView::SignIn, Some(reason));
    false
}

pub fn open(app: &mut App, v: AuthView, reason: Option<&str>) {
    app.auth.view = Some(v);
    app.auth.reason = reason.map(|r| r.to_string());
    app.auth.error = None;
    app.auth.menu_open = false;
    app.render();
    dom::focus(match v {
        AuthView::SignIn => "si-email",
        AuthView::SignUp => "su-name",
        AuthView::Forgot => "fp-email",
        AuthView::Reset => "rp-pw",
        AuthView::Account => "acct-name",
    });
}

/// Handles `auth-*` / `acct-*` actions. Returns true if it consumed the action.
pub fn handle(app: &mut App, act: &str, arg: &str) -> bool {
    match act {
        "auth-open" => {
            let v = if arg == "up" { AuthView::SignUp } else { AuthView::SignIn };
            open(app, v, None);
        }
        "auth-switch" => {
            let v = if arg == "up" { AuthView::SignUp } else { AuthView::SignIn };
            // Carry the email over between the two forms.
            let email = input_value(if v == AuthView::SignUp { "si-email" } else { "su-email" });
            let reason = app.auth.reason.clone();
            open(app, v, reason.as_deref());
            dom::set_input_value(if v == AuthView::SignUp { "su-email" } else { "si-email" }, &email);
        }
        "auth-forgot" => {
            let email = input_value("si-email");
            app.auth.sent_to = None;
            open(app, AuthView::Forgot, None);
            dom::set_input_value("fp-email", &email);
        }
        "auth-forgot-send" => {
            if app.auth.busy {
                return true;
            }
            let email = input_value("fp-email");
            if email.trim().is_empty() {
                set_error(app, "Enter your email address.");
            } else {
                submit(app, "/api/forgot-password", serde_json::json!({ "email": email }));
            }
        }
        "auth-reset" => {
            if app.auth.busy {
                return true;
            }
            let pw = input_value("rp-pw");
            let min = app.auth.min_password.max(1);
            match app.auth.reset_token.clone() {
                None => set_error(app, "This reset link is invalid or has expired. Request a new one."),
                Some(_) if pw.chars().count() < min => set_error(app, &format!("Passwords need at least {min} characters.")),
                Some(token) => submit(app, "/api/reset-password", serde_json::json!({ "token": token, "password": pw })),
            }
        }
        "auth-close" => {
            app.auth.view = None;
            app.auth.reason = None;
            app.render();
        }
        "close-modal" if app.auth.view.is_some() || app.auth.menu_open => {
            app.auth.view = None;
            app.auth.menu_open = false;
            app.render();
        }
        "auth-provider" => {
            let back = format!("{}{}", dom::window().location().pathname().unwrap_or_else(|_| "/".into()), dom::window().location().search().unwrap_or_default());
            let _ = dom::window().location().set_href(&format!("/auth/{}/start?redirect={}", dom::encode_uri(arg), dom::encode_uri(&back)));
        }
        "auth-sign-in" => {
            if app.auth.busy {
                return true;
            }
            let email = input_value("si-email");
            let pw = input_value("si-pw");
            if email.trim().is_empty() || pw.is_empty() {
                set_error(app, "Enter your email and password.");
            } else {
                submit(app, "/api/sign-in", serde_json::json!({ "email": email, "password": pw }));
            }
        }
        "auth-sign-up" => {
            if app.auth.busy {
                return true;
            }
            let name = input_value("su-name");
            let email = input_value("su-email");
            let pw = input_value("su-pw");
            let min = app.auth.min_password.max(1);
            if name.trim().is_empty() || email.trim().is_empty() {
                set_error(app, "Enter a player name and an email address.");
            } else if pw.chars().count() < min {
                set_error(app, &format!("Passwords need at least {min} characters."));
            } else {
                submit(app, "/api/sign-up", serde_json::json!({ "name": name, "email": email, "password": pw }));
            }
        }
        "auth-show-pw" => {
            if let Some(i) = by_id(arg).and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                let show = i.type_() == "password";
                i.set_type(if show { "text" } else { "password" });
                if let Some(b) = by_id(&format!("{arg}-eye")) {
                    let _ = b.set_attribute("aria-pressed", if show { "true" } else { "false" });
                }
            }
        }
        "acct-menu" => {
            app.auth.menu_open = !app.auth.menu_open;
            app.render();
        }
        "acct-open" => open(app, AuthView::Account, None),
        "acct-save" => {
            if !app.auth.busy {
                submit(app, "/api/account", serde_json::json!({ "name": input_value("acct-name") }));
            }
        }
        "acct-sign-out" => sign_out(),
        "auth-resume" => {
            app.auth.replaced = false;
            net::reconnect();
            app.connected = false;
            app.render();
        }
        _ => return false,
    }
    true
}

fn set_error(app: &mut App, msg: &str) {
    app.auth.error = Some(msg.to_string());
    render_dialog(app);
}

// ------------------------------------------------------------------ rendering

/// Right side of the top bar: account menu, or sign in / sign up.
pub fn profile_html(app: &App) -> String {
    if !app.auth.loaded {
        return "<div class=\"profile\"></div>".into();
    }
    match &app.auth.user {
        Some(u) => {
            let initial = u.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            let menu = if app.auth.menu_open {
                format!(
                    "<div class=\"acct-pop\" id=\"acct-pop\"><div class=\"ap-head\"><b>{}</b><span>{}</span></div><button class=\"mi\" data-act=\"acct-open\">Account settings</button><button class=\"mi\" data-act=\"acct-sign-out\">Sign out</button></div>",
                    esc(&u.name),
                    esc(u.email.as_deref().unwrap_or(""))
                )
            } else {
                String::new()
            };
            format!(
                "<div class=\"profile\"><button class=\"acct-btn{}\" id=\"acct-btn\" data-act=\"acct-menu\" aria-haspopup=\"menu\" aria-expanded=\"{}\"><span class=\"avatar\">{}</span><span class=\"acct-name\">{}</span><span class=\"chev\"></span></button>{menu}</div>",
                if app.auth.menu_open { " on" } else { "" },
                app.auth.menu_open,
                esc(&initial),
                esc(&u.name)
            )
        }
        None => "<div class=\"profile\"><button class=\"btn ghost sm\" data-act=\"auth-open\" data-arg=\"in\">Sign in</button><button class=\"btn sm\" data-act=\"auth-open\" data-arg=\"up\">Sign up</button></div>".into(),
    }
}

fn provider_buttons(app: &App) -> String {
    if app.auth.providers.is_empty() {
        return String::new();
    }
    let btns: String = app
        .auth
        .providers
        .iter()
        .map(|p| format!("<button class=\"prov\" data-act=\"auth-provider\" data-arg=\"{}\">{}<span>{}</span></button>", esc(&p.id), provider_mark(&p.id), esc(&p.label)))
        .collect();
    format!("<div class=\"provs n{}\">{btns}</div><div class=\"or\"><span>or</span></div>", app.auth.providers.len())
}

/// Small monochrome brand marks (simplified glyphs, not the official logos).
fn provider_mark(id: &str) -> &'static str {
    match id {
        "google" => r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M21.6 12.2c0-.7-.1-1.4-.2-2H12v3.8h5.4a4.6 4.6 0 0 1-2 3v2.5h3.2c1.9-1.7 3-4.3 3-7.3zM12 22c2.7 0 5-.9 6.6-2.4l-3.2-2.5c-.9.6-2 1-3.4 1-2.6 0-4.8-1.8-5.6-4.1H3.1v2.6A10 10 0 0 0 12 22zm-5.6-8a6 6 0 0 1 0-3.9V7.5H3.1a10 10 0 0 0 0 9zM12 5.9c1.5 0 2.8.5 3.8 1.5l2.9-2.9A10 10 0 0 0 3.1 7.5l3.3 2.6C7.2 7.7 9.4 5.9 12 5.9z"/></svg>"#,
        "discord" => r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M19.3 5.3A17 17 0 0 0 15 4l-.5 1.1a16 16 0 0 0-5 0L9 4a17 17 0 0 0-4.3 1.3C2 9.4 1.3 13.4 1.6 17.3A17 17 0 0 0 6.9 20l1.1-1.8c-.6-.2-1.2-.5-1.7-.9l.4-.3a12 12 0 0 0 10.6 0l.4.3c-.5.4-1.1.7-1.7.9l1.1 1.8a17 17 0 0 0 5.3-2.7c.4-4.5-.7-8.5-3.1-12zM8.7 14.9c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1zm6.6 0c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1z"/></svg>"#,
        "twitch" => r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M4.3 2 3 5.4V19h4.6v3h2.6l3-3H17l5-5V2zm16 11.1-2.9 2.9h-4.6l-2.5 2.5V16H6.4V3.7h13.9zM17.4 7h-1.7v5h1.7zm-4.6 0h-1.7v5h1.7z"/></svg>"#,
        _ => "",
    }
}

fn pw_field(id: &str, autocomplete: &str, hint: &str) -> String {
    format!(
        "<label class=\"fld\"><span class=\"fl\">Password{}</span><span class=\"pw\"><input id=\"{id}\" type=\"password\" autocomplete=\"{autocomplete}\" maxlength=\"256\" data-enter=\"{}\"><button type=\"button\" class=\"eye\" id=\"{id}-eye\" data-act=\"auth-show-pw\" data-arg=\"{id}\" aria-label=\"Show password\" aria-pressed=\"false\">{}</button></span></label>",
        if hint.is_empty() { String::new() } else { format!("<em>{}</em>", esc(hint)) },
        match id {
            "su-pw" => "auth-sign-up",
            "rp-pw" => "auth-reset",
            _ => "auth-sign-in",
        },
        icon("eye")
    )
}

fn dialog_html(app: &App) -> String {
    let Some(view) = app.auth.view else { return String::new() };
    let reason = app.auth.reason.as_ref().map(|r| format!("<p class=\"au-reason\">{}</p>", esc(r))).unwrap_or_default();
    let body = match view {
        AuthView::SignIn => format!(
            r#"<h2 class="au-t">Sign in to Rift Online</h2>{reason}{provs}
<label class="fld"><span class="fl">Email address</span><input id="si-email" type="email" autocomplete="email" maxlength="254" data-enter="auth-sign-in"></label>
{pw}
<div class="au-forgot"><button class="lnk" data-act="auth-forgot">Forgot password?</button></div>
<div id="au-err" class="au-err" role="alert"></div>
<button class="btn primary au-go" id="au-go" data-act="auth-sign-in">Continue</button>
<div class="au-foot">New to Rift Online? <button class="lnk on" data-act="auth-switch" data-arg="up">Create an account</button></div>"#,
            provs = provider_buttons(app),
            pw = pw_field("si-pw", "current-password", ""),
        ),
        AuthView::SignUp => format!(
            r#"<h2 class="au-t">Create your account</h2>{reason}{provs}
<label class="fld"><span class="fl">Player name<em>Shown to other players</em></span><input id="su-name" autocomplete="nickname" maxlength="24" data-enter="auth-sign-up"></label>
<label class="fld"><span class="fl">Email address</span><input id="su-email" type="email" autocomplete="email" maxlength="254" data-enter="auth-sign-up"></label>
{pw}
<div id="au-err" class="au-err" role="alert"></div>
<button class="btn primary au-go" id="au-go" data-act="auth-sign-up">Continue</button>
<div class="au-foot">Already have an account? <button class="lnk on" data-act="auth-switch" data-arg="in">Sign in</button></div>"#,
            provs = provider_buttons(app),
            pw = pw_field("su-pw", "new-password", &format!("At least {} characters", app.auth.min_password.max(1))),
        ),
        AuthView::Forgot => match &app.auth.sent_to {
            Some(email) => format!(
                r#"<h2 class="au-t">Check your email</h2>
<p class="au-note">If an account uses <b>{}</b>, we sent it a link to choose a new password. The link works for 30 minutes.</p>
<p class="au-note dim">No email? Check your spam folder, or try again in a minute.</p>
<button class="btn primary au-go" data-act="auth-switch" data-arg="in">Back to sign in</button>
<div class="au-foot"><button class="lnk on" data-act="auth-forgot">Use a different email</button></div>"#,
                esc(email)
            ),
            None => r#"<h2 class="au-t">Reset your password</h2>
<p class="au-note">Enter the email address of your account and we'll send you a link to choose a new password.</p>
<label class="fld"><span class="fl">Email address</span><input id="fp-email" type="email" autocomplete="email" maxlength="254" data-enter="auth-forgot-send"></label>
<div id="au-err" class="au-err" role="alert"></div>
<button class="btn primary au-go" id="au-go" data-act="auth-forgot-send">Send reset link</button>
<div class="au-foot">Remembered it? <button class="lnk on" data-act="auth-switch" data-arg="in">Sign in</button></div>"#
                .to_string(),
        },
        AuthView::Reset => format!(
            r#"<h2 class="au-t">Choose a new password</h2>
{pw}
<div id="au-err" class="au-err" role="alert"></div>
<button class="btn primary au-go" id="au-go" data-act="auth-reset">Set password and sign in</button>
<div class="au-foot">Link not working? <button class="lnk on" data-act="auth-forgot">Request a new one</button></div>"#,
            pw = pw_field("rp-pw", "new-password", &format!("At least {} characters", app.auth.min_password.max(1))),
        ),
        AuthView::Account => {
            let Some(u) = &app.auth.user else { return String::new() };
            let mut methods: Vec<String> = vec![];
            if u.has_password {
                methods.push("Email and password".into());
            }
            for p in &u.providers {
                let label = app.auth.providers.iter().find(|x| &x.id == p).map(|x| x.label.clone()).unwrap_or_else(|| p.clone());
                methods.push(label);
            }
            format!(
                r#"<h2 class="au-t">Account</h2>
<label class="fld"><span class="fl">Player name<em>3 to 24 characters</em></span><input id="acct-name" maxlength="24" value="{name}" data-enter="acct-save"></label>
<dl class="au-dl"><dt>Email</dt><dd>{email}</dd><dt>Sign-in</dt><dd>{methods}</dd></dl>
<div id="au-err" class="au-err" role="alert"></div>
<div class="au-row"><button class="btn primary" id="au-go" data-act="acct-save">Save</button><button class="btn ghost" data-act="acct-sign-out">Sign out</button></div>"#,
                name = esc(&u.name),
                email = esc(u.email.as_deref().unwrap_or("None")),
                methods = esc(&methods.join(", ")),
            )
        }
    };
    format!(
        "<div class=\"modal-bg au-bg\" data-act=\"auth-close\"><div class=\"dialog au\" data-act=\"noop\" role=\"dialog\" aria-modal=\"true\"><button class=\"au-x\" data-act=\"auth-close\" aria-label=\"Close\">{}</button>{body}</div></div>",
        icon("close")
    )
}

fn replaced_html(app: &App) -> String {
    if !app.auth.replaced {
        return String::new();
    }
    "<div class=\"modal-bg dark\"><div class=\"dialog au\" data-act=\"noop\"><h2 class=\"au-t\">Open in another tab</h2><p class=\"au-reason\">Your account connected from another tab or device, so this one stopped.</p><button class=\"btn primary au-go\" data-act=\"auth-resume\">Use here</button></div></div>".into()
}

/// The dialog lives in its own container, so re-rendering a screen never
/// clears what the user typed. Only the error line and busy state update in place.
pub fn render_dialog(app: &App) {
    let html = if app.auth.replaced { replaced_html(app) } else { dialog_html(app) };
    set_html("auth", &html);
    set_html("au-err", &app.auth.error.as_deref().map(esc).unwrap_or_default());
    if let Some(b) = by_id("au-go") {
        if app.auth.busy {
            let _ = b.set_attribute("disabled", "");
        } else {
            let _ = b.remove_attribute("disabled");
        }
    }
}
