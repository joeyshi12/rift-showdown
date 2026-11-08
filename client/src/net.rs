//! WebSocket connection with automatic reconnect.

use crate::dom;
use rift_shared::protocol::{ClientMsg, ServerMsg};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

type Hello = Rc<dyn Fn() -> ClientMsg>;

thread_local! {
    static SOCK: RefCell<Option<WebSocket>> = const { RefCell::new(None) };
    static OUTBOX: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static RETRY: RefCell<i32> = const { RefCell::new(0) };
    static HELLO: RefCell<Option<Hello>> = const { RefCell::new(None) };
    /// Set when another tab took over this account; stops automatic reconnects.
    static STOPPED: RefCell<bool> = const { RefCell::new(false) };
}

pub fn ws_url() -> String {
    let loc = dom::window().location();
    let proto = if loc.protocol().unwrap_or_default() == "https:" { "wss" } else { "ws" };
    format!("{proto}://{}/ws", loc.host().unwrap_or_default())
}

pub fn is_open() -> bool {
    SOCK.with(|s| s.borrow().as_ref().map(|w| w.ready_state() == WebSocket::OPEN).unwrap_or(false))
}

/// Open the socket. `hello` is sent first on every connect.
pub fn connect(hello: impl Fn() -> ClientMsg + 'static) {
    let hello: Hello = Rc::new(hello);
    HELLO.with(|h| *h.borrow_mut() = Some(hello.clone()));
    open(hello);
}

/// Close the current socket without triggering the retry path.
fn drop_socket() {
    SOCK.with(|s| {
        if let Some(w) = s.borrow_mut().take() {
            w.set_onclose(None);
            w.set_onmessage(None);
            let _ = w.close();
        }
    });
}

/// Reconnect now, e.g. after signing in or out so the server sees the new session.
pub fn reconnect() {
    STOPPED.with(|s| *s.borrow_mut() = false);
    drop_socket();
    OUTBOX.with(|o| o.borrow_mut().clear());
    crate::app::on_connection(false);
    if let Some(h) = HELLO.with(|h| h.borrow().clone()) {
        open(h);
    }
}

/// Stop for good (until `reconnect`).
pub fn stop() {
    STOPPED.with(|s| *s.borrow_mut() = true);
    drop_socket();
    crate::app::on_connection(false);
}

fn open(hello: Hello) {
    let Ok(ws) = WebSocket::new(&ws_url()) else {
        schedule_retry(hello);
        return;
    };
    let h2 = hello.clone();
    let ws2 = ws.clone();
    let onopen = Closure::<dyn FnMut()>::new(move || {
        RETRY.with(|r| *r.borrow_mut() = 0);
        let _ = ws2.send_with_str(&serde_json::to_string(&h2()).unwrap());
        let queued: Vec<String> = OUTBOX.with(|o| o.borrow_mut().drain(..).collect());
        for m in queued {
            let _ = ws2.send_with_str(&m);
        }
        crate::app::on_connection(true);
    });
    let onmsg = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(t) = e.data().as_string() {
            match serde_json::from_str::<ServerMsg>(&t) {
                Ok(m) => crate::app::on_server(m),
                Err(err) => dom::log(&format!("bad server message: {err}")),
            }
        }
    });
    let h3 = hello.clone();
    let onclose = Closure::<dyn FnMut(CloseEvent)>::new(move |_e: CloseEvent| {
        crate::app::on_connection(false);
        schedule_retry(h3.clone());
    });
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    ws.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onopen.forget();
    onmsg.forget();
    onclose.forget();
    SOCK.with(|s| *s.borrow_mut() = Some(ws));
}

fn schedule_retry(hello: Hello) {
    if STOPPED.with(|s| *s.borrow()) {
        return;
    }
    let n = RETRY.with(|r| {
        let mut r = r.borrow_mut();
        *r += 1;
        *r
    });
    let delay = (500 * n).min(5000);
    dom::set_timeout(delay, move || {
        // Skip if a reconnect already opened a newer socket meanwhile.
        let dead = SOCK.with(|s| s.borrow().as_ref().map(|w| w.ready_state() == WebSocket::CLOSED).unwrap_or(true));
        if dead && !STOPPED.with(|s| *s.borrow()) {
            open(hello)
        }
    });
}

pub fn send(m: &ClientMsg) {
    let text = serde_json::to_string(m).unwrap();
    let sent = SOCK.with(|s| match s.borrow().as_ref() {
        Some(w) if w.ready_state() == WebSocket::OPEN => w.send_with_str(&text).is_ok(),
        _ => false,
    });
    if !sent {
        OUTBOX.with(|o| o.borrow_mut().push(text));
    }
}
