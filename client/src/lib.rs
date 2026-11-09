//! Rift Showdown browser client (Rust → WebAssembly).

pub mod app;
pub mod board;
pub mod dom;
pub mod gfx;
pub mod name;
pub mod route;
pub mod icons;
pub mod math;
pub mod net;
pub mod ui_game;
pub mod ui_menu;

use app::{with_app, App, APP};
use rift_shared::cards::CardDb;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Event, KeyboardEvent, MouseEvent, PointerEvent};

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        match load_cards().await {
            Ok(db) => boot(db),
            Err(e) => {
                dom::force_html("screen", &format!("<div class=\"fatal\"><h1>Could not load card data</h1><p>{}</p><button class=\"btn\" onclick=\"location.reload()\">Reload</button></div>", dom::esc(&e)));
            }
        }
    });
}

async fn load_cards() -> Result<CardDb, String> {
    let resp = wasm_bindgen_futures::JsFuture::from(dom::window().fetch_with_str("/cards.json")).await.map_err(|e| format!("{e:?}"))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "bad response")?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let text = wasm_bindgen_futures::JsFuture::from(resp.text().map_err(|_| "no body")?).await.map_err(|_| "read failed")?;
    CardDb::from_json(&text.as_string().unwrap_or_default())
}

fn boot(db: CardDb) {
    APP.with(|a| *a.borrow_mut() = Some(App::new(db)));
    let doc = dom::document();
    let target: web_sys::EventTarget = doc.clone().into();
    dom::listen::<MouseEvent>(&target, "click", |e| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        if ui_game::menu_just_opened() && el.closest("#ctx").ok().flatten().is_some() {
            return;
        }
        let Ok(Some(hit)) = el.closest("[data-act]") else {
            // clicking outside an open context menu closes it
            if el.closest("#ctx").ok().flatten().is_none() && el.id() != "table" && el.closest("[data-hand]").ok().flatten().is_none() {
                with_app(|a| {
                    if let Some(g) = a.game.as_mut() {
                        if g.menu.is_some() {
                            g.menu = None;
                            a.render();
                        }
                    }
                });
            }
            return;
        };
        if hit.has_attribute("disabled") {
            return;
        }
        let act = hit.get_attribute("data-act").unwrap_or_default();
        let arg = hit.get_attribute("data-arg").unwrap_or_default();
        // A click on a dialog body must not trigger the backdrop's close action.
        if act == "noop" {
            return;
        }
        e.stop_propagation();
        with_app(|a| a.on_click(&act, &arg, &e));
    });
    dom::listen::<Event>(&target, "change", |e| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        if let Some(k) = el.get_attribute("data-change") {
            with_app(|a| a.on_change(&k, &el));
        }
    });
    dom::listen::<Event>(&target, "input", |e| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        if let Some(k) = el.get_attribute("data-input") {
            let v = dom::input_value(&el.id());
            with_app(|a| a.on_input(&k, &v));
        }
    });
    dom::listen::<KeyboardEvent>(&target, "keydown", |e| {
        with_app(|a| a.on_key(&e));
    });
    for (ty, kind) in [("pointerdown", "down"), ("pointermove", "move"), ("pointerup", "up"), ("pointercancel", "cancel")] {
        dom::listen::<PointerEvent>(&target, ty, move |e| {
            with_app(|a| a.on_pointer(kind, &e));
        });
    }
    // Hover previews for DOM cards outside the game (editor, sealed pool).
    dom::listen::<MouseEvent>(&target, "mouseover", |e| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        let Some(code) = el.closest("[data-code]").ok().flatten().and_then(|c| c.get_attribute("data-code")) else { return };
        with_app(|a| {
            if a.screen == app::Screen::Editor && !code.is_empty() {
                let html = ui_menu::card_preview(&a.db, &code, "");
                dom::set_html("preview", &html);
            }
        });
    });
    // Prevent the browser context menu on the table so right-drag feels natural.
    if let Some(c) = dom::by_id("table") {
        let t: web_sys::EventTarget = c.into();
        dom::listen::<MouseEvent>(&t, "contextmenu", |e| e.prevent_default());
    }
    let win: web_sys::EventTarget = dom::window().into();
    dom::listen::<Event>(&win, "popstate", |_| {
        with_app(route::on_popstate);
    });
    with_app(|a| {
        route::boot(a);
        a.render();
    });
    net::connect(app::hello);
    // Animation loop.
    let f: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let g = f.clone();
    *g.borrow_mut() = Some(Closure::new(move || {
        with_app(|a| a.frame());
        let _ = dom::window().request_animation_frame(f.borrow().as_ref().unwrap().as_ref().unchecked_ref());
    }));
    let _ = dom::window().request_animation_frame(g.borrow().as_ref().unwrap().as_ref().unchecked_ref());
    // Keep the socket alive and refresh queue/lobby timers.
    let ping = Closure::<dyn FnMut()>::new(|| {
        if net::is_open() {
            net::send(&rift_shared::protocol::ClientMsg::Ping);
        }
    });
    let _ = dom::window().set_interval_with_callback_and_timeout_and_arguments_0(ping.as_ref().unchecked_ref(), 20000);
    ping.forget();
}

/// Test hook: expose a JSON snapshot of high-level UI state for E2E tests.
#[wasm_bindgen]
pub fn debug_state() -> String {
    with_app(|a| {
        let g = a.game.as_ref().map(|g| {
            serde_json::json!({
                "stage": format!("{:?}", g.view.game.stage),
                "turn": g.view.game.turn,
                "phase": g.view.game.phase.name(),
                "active": g.view.game.active,
                "me": g.me(),
                "local": g.is_local(),
                "spectator": g.spectator(),
                "scores": g.view.game.seats.iter().map(|s| s.score).collect::<Vec<_>>(),
                "mode": format!("{:?}", g.view.game.options.mode),
                "first": g.view.game.first,
                "nbf": g.view.game.battlefields.len(),
                "conceded": g.view.game.seats.iter().map(|s| s.conceded).collect::<Vec<_>>(),
                "hands": (0..g.view.game.num_seats()).map(|s| g.view.game.hand(s).len()).collect::<Vec<_>>(),
                "visible_hands": (0..g.view.game.num_seats()).map(|s| g.view.game.hand(s).iter().filter(|&&id| !g.view.game.cards[id as usize].code.is_empty()).count()).collect::<Vec<_>>(),
                "hand": g.view.game.hand(g.me()).len(),
                "base": g.view.game.pile(rift_shared::game::Zone::Base(g.me())).len(),
                "runes": g.view.game.pile(rift_shared::game::Zone::Runes(g.me())).len(),
                "winner": g.view.game.winner,
                "can_rewind": g.view.can_rewind,
                "log": g.view.game.log.last().map(|l| l.text.clone()),
                "targeting": g.targeting.as_ref().map(|t| format!("{:?}", t.kind)),
                "targets": g.targeting.as_ref().map(|t| t.zones.len()).unwrap_or(0),
                "textures_pending": a.renderer.as_ref().map(|r| r.pending_images()).unwrap_or(0),
                "scene_cards": a.scene.anims.len(),
            })
        });
        serde_json::json!({
            "screen": format!("{:?}", a.screen),
            "connected": a.connected,
            "name": a.name,
            "name_prompt": a.names.pending.is_some(),
            "room": a.room.as_ref().map(|r| r.code.clone()),
            "in_game": a.room.as_ref().map(|r| r.in_game),
            "players": a.room.as_ref().map(|r| r.players.len()),
            "decks": a.decks.len(),
            "queue": a.queue.is_some(),
            "chat": a.chat.len(),
            "game": g,
        })
        .to_string()
    })
    .unwrap_or_else(|| "{}".into())
}

/// Test hook: screen position (CSS px) of a zone's centre, e.g. `{"Base":0}`.
#[wasm_bindgen]
pub fn debug_zone_point(zone_json: &str) -> Vec<f64> {
    with_app(|a| {
        let z: rift_shared::game::Zone = serde_json::from_str(zone_json).ok()?;
        let g = a.game.as_ref()?;
        let r = a.renderer.as_ref()?;
        let rect = g.layout().rect_for_zone(z)?;
        r.to_screen(math::v3(rect.cx, 0.0, rect.cz)).map(|(x, y)| vec![x, y])
    })
    .flatten()
    .unwrap_or_default()
}
