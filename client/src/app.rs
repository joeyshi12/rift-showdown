//! Application state, event routing and game action dispatch.

use crate::board::{Layout, Scene};
use crate::dom::{self, by_id, input_value};
use crate::gfx::Renderer;
use crate::net;
use rift_shared::cards::CardDb;
use rift_shared::decklist::{Deck, DeckRules};
use rift_shared::game::{Action, CardId, Pos, Stage, Zone};
use rift_shared::protocol::*;
use rift_shared::sealed::{self, SealedPool};
use rift_shared::session::{Format, Match, MatchAction, MatchConfig, MatchStage, MatchView};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, KeyboardEvent, MouseEvent, PointerEvent};

thread_local! {
    pub static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Duel,
    Solo,
    Sealed,
    Decks,
    Rooms,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Home(Tab),
    Editor,
    Lobby,
    Game,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedDeck {
    pub id: String,
    pub deck: Deck,
    pub sealed: Option<SealedRef>,
    #[serde(default)]
    pub updated: f64,
}

#[derive(Default)]
pub struct EditorState {
    pub deck_id: Option<String>,
    pub deck: Deck,
    pub sealed: Option<SealedRef>,
    pub pool: Option<HashMap<String, u32>>,
    pub query: String,
    pub ftype: String,
    pub fdomain: String,
    pub fset: String,
    pub fcost: String,
    pub limit: usize,
    pub side_mode: bool,
    pub dirty: bool,
}

#[derive(Default)]
pub struct SealedState {
    pub pool: Option<SealedPool>,
    pub sets: Vec<String>,
    pub seed: String,
}

pub enum Source {
    Online,
    Local(Box<Match>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuKind {
    Card(CardId),
    Zone(Zone),
    Battlefield(u8),
    Score(u8),
    Game,
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub kind: MenuKind,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    Pile(Zone),
    Tokens,
    Help,
    Sideboard,
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub card: CardId,
    pub sx: f64,
    pub sy: f64,
    pub x: f64,
    pub y: f64,
    pub active: bool,
    pub from_hand: bool,
}

/// An action waiting for the player to click where it goes (Master Duel style).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    /// Move a unit or gear between its base and battlefields.
    Move,
    /// Play a card from hand or the champion zone to base or a battlefield.
    Play,
    /// Hide a card face down at a battlefield.
    Hide,
}

impl TargetKind {
    pub fn verb(&self) -> &'static str {
        match self {
            TargetKind::Move => "move",
            TargetKind::Play => "play",
            TargetKind::Hide => "hide",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Targeting {
    pub card: CardId,
    pub kind: TargetKind,
    /// Highlighted destinations.
    pub zones: Vec<Zone>,
    /// Destination under the pointer.
    pub hover: Option<Zone>,
    /// Where the card was when the choice started; if it leaves, the choice is dropped.
    pub from: Zone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Log,
    Chat,
    Hidden,
}

pub struct GameUi {
    pub src: Source,
    pub view: MatchView,
    pub acting: u8,
    pub selected: Option<CardId>,
    pub hover: Option<CardId>,
    pub hover_code: Option<String>,
    pub menu: Option<Menu>,
    pub modal: Option<Modal>,
    pub mull_sel: Vec<CardId>,
    pub drag: Option<Drag>,
    pub drop_zone: Option<Zone>,
    pub panel: Panel,
    pub autopay: bool,
    pub side_out: Option<String>,
    pub side_in: Option<String>,
    pub ping: Option<(CardId, f64)>,
    pub last_ping: u32,
    pub token_to: String,
    pub sealed_ref: Option<SealedRef>,
    pub unread: usize,
    pub targeting: Option<Targeting>,
}

impl GameUi {
    pub fn is_local(&self) -> bool {
        matches!(self.src, Source::Local(_))
    }
    pub fn spectator(&self) -> bool {
        !self.is_local() && self.view.viewer.is_none()
    }
    pub fn me(&self) -> u8 {
        if self.is_local() {
            self.acting
        } else {
            self.view.viewer.unwrap_or(0)
        }
    }
    pub fn layout(&self) -> Layout {
        let g = &self.view.game;
        Layout { nbf: g.battlefields.len(), me: self.me(), seats: g.num_seats(), mode: g.options.mode }
    }
}

pub struct App {
    pub db: Rc<CardDb>,
    pub screen: Screen,
    pub name: String,
    pub my_id: u64,
    pub token: Option<String>,
    pub connected: bool,
    pub decks: Vec<SavedDeck>,
    pub editor: EditorState,
    pub sealed: SealedState,
    pub room: Option<RoomView>,
    pub chat: Vec<ChatLine>,
    pub rooms: Vec<RoomSummary>,
    pub queue: Option<QueueStatus>,
    pub toasts: Vec<(String, f64, bool)>,
    pub game: Option<GameUi>,
    pub renderer: Option<Renderer>,
    pub scene: Scene,
    pub last_frame: f64,
    pub badges: HashMap<String, HtmlElement>,
    pub import_open: bool,
    /// Deck shown on the home screen and used for matchmaking and goldfish.
    pub home_deck: Option<String>,
    pub dirty: bool,
    pub frames: u64,
    pub auth: crate::account::AuthUi,
}

const DECKS_KEY: &str = "rift.decks.v1";
const AUTOPAY_KEY: &str = "rift.autopay";
const HOME_DECK_KEY: &str = "rift.home_deck";

/// Riot's preconstructed decks, as a first visit gets them.
fn riot_decks() -> Vec<SavedDeck> {
    rift_shared::precons::all().iter().map(|p| SavedDeck { id: new_id() + &p.legend, deck: p.deck(), sealed: None, updated: 0.0 }).collect()
}

/// Legends whose generated starter decks older versions added on a first visit.
const GENERATED_STARTER_LEGENDS: [&str; 4] = ["OGN-251", "OGN-253", "OGN-247", "OGN-255"];

/// Older versions built starter decks themselves instead of using Riot's lists.
/// Swap the ones the player never touched for Riot's decks. A deck counts as
/// untouched when it still has the first-visit marks: no save time, no sealed
/// pool, an id ending in its legend code and the generated "<legend> Starter"
/// name. Returns whether anything changed.
fn replace_generated_starters(db: &CardDb, decks: &mut Vec<SavedDeck>) -> bool {
    let generated = |sd: &SavedDeck| {
        GENERATED_STARTER_LEGENDS.iter().any(|l| {
            sd.id.ends_with(*l) && sd.updated == 0.0 && sd.sealed.is_none() && sd.deck.legend.as_deref() == Some(*l) && db.get(l).map(|d| sd.deck.name == format!("{} Starter", d.name)).unwrap_or(false)
        })
    };
    let before = decks.len();
    decks.retain(|sd| !generated(sd));
    if decks.len() == before {
        return false;
    }
    for r in riot_decks() {
        if !decks.iter().any(|d| d.deck.legend == r.deck.legend && d.deck.name == r.deck.name) {
            decks.push(r);
        }
    }
    true
}

pub fn new_id() -> String {
    format!("{:x}{:x}", (js_sys::Math::random() * 1e12) as u64, (dom::now() * 1000.0) as u64)
}

impl App {
    pub fn new(db: CardDb) -> App {
        let db = Rc::new(db);
        let name = "Player".to_string();
        let mut decks: Vec<SavedDeck> = dom::local_get(DECKS_KEY).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if decks.is_empty() {
            decks = riot_decks();
        } else if replace_generated_starters(&db, &mut decks) {
            dom::local_set(DECKS_KEY, &serde_json::to_string(&decks).unwrap_or_default());
        }
        let app = App {
            db,
            screen: Screen::Home(Tab::Duel),
            name,
            my_id: 0,
            token: None,
            connected: false,
            decks,
            editor: EditorState { limit: 120, ..Default::default() },
            sealed: SealedState { sets: sealed::default_sets("OGN"), seed: String::new(), pool: None },
            room: None,
            chat: vec![],
            rooms: vec![],
            queue: None,
            toasts: vec![],
            game: None,
            renderer: None,
            scene: Scene::default(),
            last_frame: dom::now(),
            badges: HashMap::new(),
            import_open: false,
            home_deck: dom::local_get(HOME_DECK_KEY),
            dirty: true,
            frames: 0,
            auth: Default::default(),
        };
        app.save_decks();
        app
    }

    /// The home deck: the one the player picked, else the first non-sealed deck.
    pub fn home_deck(&self) -> Option<&SavedDeck> {
        let picked = self.home_deck.as_deref().and_then(|id| self.decks.iter().find(|d| d.id == id && d.sealed.is_none()));
        picked.or_else(|| self.decks.iter().find(|d| d.sealed.is_none()))
    }

    pub fn set_home_deck(&mut self, id: &str) {
        self.home_deck = Some(id.to_string());
        dom::local_set(HOME_DECK_KEY, id);
    }

    pub fn save_decks(&self) {
        dom::local_set(DECKS_KEY, &serde_json::to_string(&self.decks).unwrap_or_default());
    }

    pub fn toast(&mut self, msg: &str, err: bool) {
        self.toasts.retain(|t| t.0 != msg);
        self.toasts.push((msg.to_string(), dom::now() + 3500.0, err));
        if self.toasts.len() > 2 {
            self.toasts.remove(0);
        }
        self.render_toasts();
    }

    pub fn go(&mut self, s: Screen) {
        if s == Screen::Game {
            self.toasts.clear();
        }
        self.screen = s;
        self.dirty = true;
        if let Screen::Home(Tab::Rooms) = s {
            net::send(&ClientMsg::ListRooms);
        }
        self.render();
    }

    pub fn deck_by_id(&self, id: &str) -> Option<&SavedDeck> {
        self.decks.iter().find(|d| d.id == id)
    }

    // ------------------------------------------------------------- server

    pub fn handle_server(&mut self, m: ServerMsg) {
        match m {
            ServerMsg::Welcome { id, token, name, signed_in } => {
                self.my_id = id;
                self.token = Some(token);
                if signed_in {
                    self.name = name.clone();
                    if let Some(u) = self.auth.user.as_mut() {
                        u.name = name;
                    }
                } else if self.auth.user.is_some() {
                    // The session ended on the server (expired or signed out elsewhere).
                    self.auth.user = None;
                    self.name = "Player".into();
                    self.toast("Your session ended. Sign in again.", true);
                }
                // Invite links: players need an account, spectators don't. Keep the
                // link in the address bar until it can be used, so signing in
                // (which reconnects) picks it up.
                if let Some(code) = dom::query_param("room") {
                    let spec = dom::query_param("spectate").is_some();
                    if signed_in || spec {
                        net::send(&ClientMsg::JoinRoom { code, spectate: spec });
                        let _ = dom::window().history().map(|h| h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/")));
                    } else {
                        crate::account::open(self, crate::account::AuthView::SignIn, Some(&format!("Sign in to join room {}.", code.to_uppercase())));
                    }
                }
                self.dirty = true;
                self.render();
            }
            ServerMsg::SignInRequired { message } => crate::account::open(self, crate::account::AuthView::SignIn, Some(&message)),
            ServerMsg::SignedOut => crate::account::on_signed_out(self),
            ServerMsg::Replaced => {
                self.auth.replaced = true;
                self.room = None;
                self.queue = None;
                if self.game.as_ref().map(|g| !g.is_local()).unwrap_or(false) {
                    self.game = None;
                    self.screen = Screen::Home(Tab::Duel);
                }
                net::stop();
                // on_connection() cannot borrow the app from inside this handler.
                self.connected = false;
                self.dirty = true;
                self.render();
            }
            ServerMsg::Error { message } => self.toast(&message, true),
            ServerMsg::Info { message } => self.toast(&message, false),
            ServerMsg::Room { room } => {
                let entering = self.room.as_ref().map(|r| r.code != room.code).unwrap_or(true);
                if entering {
                    self.chat.clear();
                }
                let in_game = room.in_game;
                self.room = Some(room);
                if !in_game {
                    if self.game.as_ref().map(|g| !g.is_local()).unwrap_or(false) {
                        self.game = None;
                    }
                    if self.screen != Screen::Editor && self.game.is_none() {
                        self.screen = Screen::Lobby;
                    }
                }
                self.queue = None;
                self.dirty = true;
                self.render();
            }
            ServerMsg::LeftRoom => {
                self.room = None;
                if self.game.as_ref().map(|g| !g.is_local()).unwrap_or(false) {
                    self.game = None;
                }
                if matches!(self.screen, Screen::Lobby | Screen::Game) && self.game.is_none() {
                    self.screen = Screen::Home(Tab::Duel);
                }
                self.dirty = true;
                self.render();
            }
            ServerMsg::Match { view } => self.on_match_view(*view),
            ServerMsg::Chat { line } => {
                self.chat.push(line);
                if self.chat.len() > 200 {
                    self.chat.remove(0);
                }
                if let Some(g) = self.game.as_mut() {
                    if g.panel != Panel::Chat {
                        g.unread += 1;
                    }
                }
                self.render_chat();
            }
            ServerMsg::ChatHistory { lines } => {
                self.chat = lines;
                self.render_chat();
            }
            ServerMsg::Rooms { rooms } => {
                self.rooms = rooms;
                self.render();
            }
            ServerMsg::Queue { status } => {
                self.queue = if status.searching { Some(status) } else { None };
                self.render();
            }
            ServerMsg::MatchFound { code } => {
                self.queue = None;
                self.toast(&format!("Match found. Room {code}."), false);
                self.render_queue();
            }
            ServerMsg::Pong => {}
        }
    }

    fn on_match_view(&mut self, view: MatchView) {
        let autopay = dom::local_get(AUTOPAY_KEY).map(|v| v == "1").unwrap_or(true);
        match self.game.as_mut() {
            Some(g) if !g.is_local() => {
                g.view = view;
            }
            _ => {
                self.game = Some(GameUi::new(Source::Online, view, autopay));
                self.scene = Scene::default();
                self.dirty = true;
            }
        }
        self.after_view_change();
        if self.screen != Screen::Game {
            self.screen = Screen::Game;
            self.dirty = true;
        }
        self.render();
    }

    fn after_view_change(&mut self) {
        let Some(g) = self.game.as_mut() else { return };
        // Drop selections that point at cards that moved out of sight.
        if let Some(s) = g.selected {
            if g.view.game.card(s).map(|c| matches!(c.zone, Zone::Void)).unwrap_or(true) {
                g.selected = None;
                g.menu = None;
            }
        }
        // A pending destination choice ends if its card moved or the turn left Play.
        if let Some(t) = &g.targeting {
            if g.view.game.card(t.card).map(|c| c.zone) != Some(t.from) || g.view.game.stage != rift_shared::game::Stage::Play {
                g.targeting = None;
            }
        }
        if let Some(p) = &g.view.game.ping {
            if p.nonce != g.last_ping {
                g.last_ping = p.nonce;
                g.ping = Some((p.card, dom::now() + 1800.0));
            }
        }
        let me = g.me();
        g.mull_sel.retain(|id| g.view.game.card(*id).map(|c| c.zone == Zone::Hand(me)).unwrap_or(false));
        // In local solo, auto-advance the acting seat through per-seat setup steps.
        if let Source::Local(m) = &g.src {
            let gs = &m.game;
            let n = gs.num_seats();
            match &gs.stage {
                Stage::ChooseBattlefield => {
                    if gs.seats[g.acting as usize].chosen_bf.is_some() {
                        if let Some(s) = (0..n).find(|&s| gs.seats[s as usize].chosen_bf.is_none() && !gs.seats[s as usize].bf_options.is_empty()) {
                            g.acting = s;
                        }
                    }
                }
                Stage::Mulligan => {
                    if gs.seats[g.acting as usize].mulligan_done {
                        if let Some(s) = (0..n).find(|&s| !gs.seats[s as usize].mulligan_done) {
                            g.acting = s;
                        }
                    }
                }
                Stage::Play => {
                    if n > 1 && g.view.game.turn != 0 && gs.stage == Stage::Play && g.view.config.solo && g.follow_active() {
                        g.acting = gs.active;
                    }
                }
                _ => {}
            }
            let v = m.view(Some(g.acting), true);
            g.view = v;
        }
    }

    pub fn act(&mut self, a: Action) {
        self.match_act(MatchAction::Game(a));
    }

    pub fn match_act(&mut self, a: MatchAction) {
        let db = self.db.clone();
        let Some(g) = self.game.as_mut() else { return };
        if g.spectator() {
            self.toast("Spectators can't take game actions.", true);
            return;
        }
        g.menu = None;
        match &mut g.src {
            Source::Online => net::send(&ClientMsg::Act { action: a }),
            Source::Local(m) => {
                let acting = g.acting;
                if let Err(e) = m.apply(&db, acting, a) {
                    self.toast(&e, true);
                }
                self.after_view_change();
            }
        }
        self.render();
    }

    // ------------------------------------------------------------- local games

    pub fn start_local(&mut self, decks: Vec<Deck>, names: Vec<String>, skip_setup: bool, sealed_ref: Option<SealedRef>) {
        let solo = true;
        let rules = if sealed_ref.is_some() { DeckRules::Sealed } else { DeckRules::Unrestricted };
        let cfg = MatchConfig { format: Format::Bo1, rules, solo, ..Default::default() };
        let seed = (js_sys::Math::random() * 1e15) as u64;
        let mut m = Match::new(&self.db, cfg, names, decks, seed);
        if skip_setup {
            let n = m.seats();
            for s in 0..n as u8 {
                if m.game.seats[s as usize].chosen_bf.is_none() {
                    if let Some(b) = m.game.seats[s as usize].bf_options.first().cloned() {
                        let _ = m.apply(&self.db, s, MatchAction::Game(Action::ChooseBattlefield { code: b }));
                    }
                }
            }
            let _ = m.apply(&self.db, 0, MatchAction::Game(Action::ChooseOrder { go_first: true }));
            for s in 0..n as u8 {
                let _ = m.apply(&self.db, s, MatchAction::Game(Action::Mulligan { cards: vec![] }));
            }
            m.game.log.push(rift_shared::game::LogLine { seat: None, turn: m.game.turn, text: "Setup skipped.".into() });
        }
        let view = m.view(Some(0), true);
        let autopay = dom::local_get(AUTOPAY_KEY).map(|v| v == "1").unwrap_or(true);
        let mut g = GameUi::new(Source::Local(Box::new(m)), view, autopay);
        g.sealed_ref = sealed_ref;
        self.game = Some(g);
        self.scene = Scene::default();
        self.after_view_change();
        self.go(Screen::Game);
    }

    pub fn leave_game(&mut self) {
        let local = self.game.as_ref().map(|g| g.is_local()).unwrap_or(true);
        self.game = None;
        self.scene = Scene::default();
        self.clear_badges();
        if local {
            self.go(if self.room.is_some() { Screen::Lobby } else { Screen::Home(Tab::Solo) });
        } else {
            net::send(&ClientMsg::LeaveRoom);
            self.room = None;
            self.go(Screen::Home(Tab::Duel));
        }
    }

    pub fn clear_badges(&mut self) {
        for (_, e) in self.badges.drain() {
            e.remove();
        }
    }

    // ------------------------------------------------------------- input routing

    pub fn on_click(&mut self, act: &str, arg: &str, ev: &MouseEvent) {
        let shift = ev.shift_key();
        if self.auth.menu_open && act != "acct-menu" {
            self.auth.menu_open = false;
            self.render();
        }
        if crate::ui_menu::handle(self, act, arg, shift) {
            return;
        }
        crate::ui_game::handle(self, act, arg, ev);
    }

    pub fn on_change(&mut self, key: &str, el: &web_sys::Element) {
        let val = if let Some(i) = el.dyn_ref::<web_sys::HtmlInputElement>() {
            if i.type_() == "checkbox" {
                if i.checked() { "1".into() } else { "0".into() }
            } else {
                i.value()
            }
        } else {
            input_value(&el.id())
        };
        crate::ui_menu::on_change(self, key, &val, el);
    }

    pub fn on_input(&mut self, key: &str, val: &str) {
        crate::ui_menu::on_input(self, key, val);
    }

    pub fn on_key(&mut self, e: &KeyboardEvent) {
        let target_is_input = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).map(|el| matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")).unwrap_or(false);
        if e.key() == "Enter" && target_is_input {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                if let Some(a) = el.get_attribute("data-enter") {
                    e.prevent_default();
                    crate::ui_menu::handle(self, &a, "", false);
                    return;
                }
            }
        }
        if target_is_input || self.screen != Screen::Game {
            if e.key() == "Escape" {
                crate::ui_menu::handle(self, "close-modal", "", false);
            }
            return;
        }
        crate::ui_game::on_key(self, e);
    }

    pub fn on_pointer(&mut self, kind: &str, e: &PointerEvent) {
        if self.screen == Screen::Game {
            crate::ui_game::on_pointer(self, kind, e);
        }
    }

    // ------------------------------------------------------------- rendering

    pub fn render(&mut self) {
        let force = self.dirty;
        self.dirty = false;
        let in_game = self.screen == Screen::Game && self.game.is_some();
        if let Some(c) = by_id("table") {
            let _ = c.set_attribute("class", if in_game { "on" } else { "" });
        }
        if !in_game {
            self.clear_badges();
        }
        match self.screen {
            Screen::Game if self.game.is_some() => crate::ui_game::render(self, force),
            Screen::Game => {
                self.screen = Screen::Home(Tab::Duel);
                crate::ui_menu::render(self, true)
            }
            _ => crate::ui_menu::render(self, force),
        }
        self.render_queue();
        self.render_toasts();
        crate::account::render_dialog(self);
    }

    pub fn render_toasts(&mut self) {
        let now = dom::now();
        self.toasts.retain(|t| t.1 > now);
        let html: String = self.toasts.iter().map(|(m, _, e)| format!("<div class=\"toast {}\">{}</div>", if *e { "err" } else { "" }, dom::esc(m))).collect();
        dom::set_html("toasts", &html);
    }

    pub fn render_chat(&mut self) {
        let html = crate::ui_menu::chat_lines(&self.chat);
        dom::set_html("chat-lines", &html);
        if let Some(el) = by_id("chat-lines") {
            el.set_scroll_top(el.scroll_height());
        }
        if self.screen == Screen::Game {
            crate::ui_game::render(self, false);
        }
    }

    pub fn render_queue(&mut self) {
        let html = match &self.queue {
            Some(q) => {
                let m = q.mode.as_ref().map(|m| format!("{}, {:?}", m.format.label(), m.rules)).unwrap_or_default();
                format!(
                    "<div class=\"queue-pop\"><div class=\"spinner\"></div><div><b>Searching for an opponent</b><div class=\"dim\">{}. Waiting {}:{:02}, {} in queue.</div></div><button class=\"btn ghost\" data-act=\"cancel-queue\">Cancel</button></div>",
                    dom::esc(&m),
                    q.waited_secs / 60,
                    q.waited_secs % 60,
                    q.in_queue
                )
            }
            None => String::new(),
        };
        dom::set_html("queue", &html);
    }

    /// Per-frame work: animate and draw the table.
    pub fn frame(&mut self) {
        let now = dom::now();
        let dt = ((now - self.last_frame) / 1000.0).clamp(0.0, 0.1) as f32;
        self.last_frame = now;
        self.frames += 1;
        if self.frames % 30 == 0 && !self.toasts.is_empty() {
            self.render_toasts();
        }
        if self.screen != Screen::Game {
            return;
        }
        let Some(g) = self.game.as_mut() else { return };
        if self.renderer.is_none() {
            if let Some(c) = by_id("table").and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok()) {
                match Renderer::new(c) {
                    Ok(r) => self.renderer = Some(r),
                    Err(e) => {
                        dom::log(&e);
                        return;
                    }
                }
            }
        }
        let Some(r) = self.renderer.as_mut() else { return };
        r.time += dt;
        let layout = g.layout();
        self.scene.ensure_table(r, &layout);
        self.scene.update(&g.view.game, &layout, dt);
        r.begin(false, layout.zoom());
        let ping = g.ping.and_then(|(c, until)| if until > now { Some(c) } else { None });
        let drag = g.drag.as_ref().filter(|d| d.active).and_then(|d| r.pick_plane(d.x, d.y, 0.9).map(|(x, z)| (d.card, x, z)));
        let drag = drag.filter(|_| !g.drag.as_ref().map(|d| d.from_hand && d.y > r.canvas.client_height() as f64 - 150.0).unwrap_or(false));
        let controllers: Vec<Option<u8>> = g.view.game.battlefields.iter().map(|b| b.controller).collect();
        let focus_bf = g.view.game.showdown.as_ref().map(|s| s.bf);
        let targets: Vec<Zone> = g.targeting.as_ref().map(|t| t.zones.clone()).unwrap_or_default();
        let target_hover = g.targeting.as_ref().and_then(|t| t.hover);
        let opts = crate::board::DrawOpts { hover: g.hover, selected: g.selected, drag, drop_zone: g.drop_zone, ping, focus_bf, controllers: &controllers, targets: &targets, target_hover, db: &self.db };
        self.scene.draw(r, &g.view.game, &layout, &opts);
        crate::ui_game::update_badges(self);
    }
}

impl GameUi {
    pub fn new(src: Source, view: MatchView, autopay: bool) -> GameUi {
        let acting = view.viewer.unwrap_or(0);
        GameUi {
            src,
            view,
            acting,
            selected: None,
            hover: None,
            hover_code: None,
            menu: None,
            modal: None,
            mull_sel: vec![],
            drag: None,
            drop_zone: None,
            panel: Panel::Log,
            autopay,
            side_out: None,
            side_in: None,
            ping: None,
            last_ping: 0,
            token_to: String::new(),
            sealed_ref: None,
            unread: 0,
            targeting: None,
        }
    }

    /// Two-seat solo play follows the active player automatically.
    pub fn follow_active(&self) -> bool {
        true
    }

    pub fn is_over(&self) -> bool {
        self.view.game.stage == Stage::Over
    }

    pub fn sideboarding(&self) -> bool {
        matches!(self.view.stage, MatchStage::Sideboarding { .. })
    }

    pub fn card_zone(&self, id: CardId) -> Option<Zone> {
        self.view.game.card(id).map(|c| c.zone)
    }
}

pub fn set_autopay(v: bool) {
    dom::local_set(AUTOPAY_KEY, if v { "1" } else { "0" });
}

/// Called by the socket layer.
pub fn on_server(m: ServerMsg) {
    with_app(|a| a.handle_server(m));
}

pub fn on_connection(up: bool) {
    with_app(|a| {
        a.connected = up;
        if !up {
            a.queue = None;
        }
        crate::ui_menu::render_status(a);
    });
}

pub fn hello() -> ClientMsg {
    with_app(|a| ClientMsg::Hello { token: a.token.clone() }).unwrap_or(ClientMsg::Hello { token: None })
}

pub fn move_action(card: CardId, to: Zone, pos: Pos) -> Action {
    Action::Move { card, to, pos }
}
