//! DOM helpers.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, HtmlElement, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement, Window};

pub fn window() -> Window {
    web_sys::window().expect("window")
}

pub fn document() -> Document {
    window().document().expect("document")
}

/// True when the player asked the OS for reduced motion. Read once per page load.
pub fn reduced_motion() -> bool {
    thread_local! {
        static RM: bool = window().match_media("(prefers-reduced-motion: reduce)").ok().flatten().map(|m| m.matches()).unwrap_or(false);
    }
    RM.with(|v| *v)
}

pub fn by_id(id: &str) -> Option<Element> {
    document().get_element_by_id(id)
}

/// Replace inner HTML only when it changed (keeps scroll/focus stable).
pub fn set_html(id: &str, html: &str) {
    if let Some(el) = by_id(id) {
        if el.get_attribute("data-h").as_deref() != Some(&hash(html)) {
            el.set_inner_html(html);
            let _ = el.set_attribute("data-h", &hash(html));
        }
    }
}

pub fn force_html(id: &str, html: &str) {
    if let Some(el) = by_id(id) {
        el.set_inner_html(html);
        let _ = el.set_attribute("data-h", &hash(html));
    }
}

fn hash(s: &str) -> String {
    format!("{:x}", rift_shared::rng::seed_from_str(s))
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn input_value(id: &str) -> String {
    let Some(el) = by_id(id) else { return String::new() };
    if let Some(i) = el.dyn_ref::<HtmlInputElement>() {
        return i.value();
    }
    if let Some(t) = el.dyn_ref::<HtmlTextAreaElement>() {
        return t.value();
    }
    if let Some(s) = el.dyn_ref::<HtmlSelectElement>() {
        return s.value();
    }
    String::new()
}

pub fn set_input_value(id: &str, v: &str) {
    let Some(el) = by_id(id) else { return };
    if let Some(i) = el.dyn_ref::<HtmlInputElement>() {
        i.set_value(v);
    } else if let Some(t) = el.dyn_ref::<HtmlTextAreaElement>() {
        t.set_value(v);
    } else if let Some(s) = el.dyn_ref::<HtmlSelectElement>() {
        s.set_value(v);
    }
}

pub fn checked(id: &str) -> bool {
    by_id(id).and_then(|e| e.dyn_into::<HtmlInputElement>().ok()).map(|i| i.checked()).unwrap_or(false)
}

pub fn focus(id: &str) {
    if let Some(e) = by_id(id).and_then(|e| e.dyn_into::<HtmlElement>().ok()) {
        let _ = e.focus();
    }
}

pub fn set_style(el: &HtmlElement, prop: &str, v: &str) {
    let _ = el.style().set_property(prop, v);
}

pub fn listen<E: JsCast + 'static>(target: &web_sys::EventTarget, ty: &str, f: impl FnMut(E) + 'static) {
    let mut f = f;
    let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
        if let Ok(ev) = e.dyn_into::<E>() {
            f(ev)
        }
    });
    let _ = target.add_event_listener_with_callback(ty, cb.as_ref().unchecked_ref());
    cb.forget();
}

pub fn local_get(key: &str) -> Option<String> {
    window().local_storage().ok().flatten().and_then(|s| s.get_item(key).ok().flatten())
}

pub fn local_set(key: &str, v: &str) {
    if let Some(s) = window().local_storage().ok().flatten() {
        let _ = s.set_item(key, v);
    }
}

pub fn now() -> f64 {
    window().performance().map(|p| p.now()).unwrap_or(0.0)
}

pub fn copy_text(s: &str) {
    let nav = window().navigator();
    let _ = nav.clipboard().write_text(s);
}

pub fn log(s: &str) {
    web_sys::console::log_1(&JsValue::from_str(s));
}

pub fn query_param(name: &str) -> Option<String> {
    let search = window().location().search().ok()?;
    let p = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    p.get(name)
}

pub fn origin() -> String {
    window().location().origin().unwrap_or_default()
}

pub fn set_timeout(ms: i32, f: impl FnOnce() + 'static) {
    let cb = Closure::once_into_js(f);
    let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms);
}

pub fn encode_uri(s: &str) -> String {
    js_sys::encode_uri_component(s).as_string().unwrap_or_default()
}
