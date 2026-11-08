//! Connection hub: sessions, rooms, lobbies, matchmaking, spectators and chat.
//! Pure state machine driven by `hello`/`handle`/`disconnect`; outgoing
//! messages go to per-connection channels, so it is testable without sockets.

use rift_shared::cards::CardDb;
use rift_shared::decklist::{Deck, DeckRules};
use rift_shared::game::Mode;
use rift_shared::protocol::*;
use rift_shared::sealed;
use rift_shared::session::{Match, MatchConfig};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;

/// Most player seats in any room (2v2 and free-for-all).
pub const MAX_PLAYERS: usize = 4;
pub const MAX_SPECTATORS: usize = 32;
pub const CHAT_HISTORY: usize = 100;
pub const SESSION_TTL: Duration = Duration::from_secs(15 * 60);
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

pub type Tx = UnboundedSender<ServerMsg>;

/// A signed-in account, as resolved from the session cookie.
#[derive(Clone, Debug)]
pub struct Identity {
    pub user_id: i64,
    pub name: String,
}

struct Client {
    name: String,
    user: Option<i64>,
    token: String,
    tx: Option<Tx>,
    conn: u64,
    room: Option<String>,
    last_seen: Instant,
}

struct Member {
    id: u64,
    deck: Option<Deck>,
    sealed: Option<SealedRef>,
    deck_errors: Vec<String>,
    ready: bool,
    /// 2v2 team (0 = A, 1 = B).
    team: u8,
    /// Left during a multiplayer match. The seat stays so seat numbers don't
    /// shift; it is removed when the match ends.
    left: bool,
}

impl Member {
    fn new(id: u64, team: u8) -> Member {
        Member { id, deck: None, sealed: None, deck_errors: vec![], ready: false, team, left: false }
    }
}

struct Room {
    code: String,
    settings: RoomSettings,
    host: u64,
    players: Vec<Member>,
    spectators: Vec<u64>,
    game: Option<Match>,
    chat: Vec<ChatLine>,
    from_queue: bool,
}

impl Room {
    fn seats(&self) -> usize {
        self.settings.mode.seats()
    }
    /// Seat of a present (not departed) player.
    fn seat_of(&self, id: u64) -> Option<usize> {
        self.players.iter().position(|m| m.id == id && !m.left)
    }
    fn active_players(&self) -> usize {
        self.players.iter().filter(|m| !m.left).count()
    }
    /// 2v2: the team with fewer players (Team A on a tie).
    fn open_team(&self) -> u8 {
        if self.settings.mode != Mode::Teams {
            return 0;
        }
        let a = self.players.iter().filter(|m| m.team == 0).count();
        let b = self.players.len() - a;
        if b < a {
            1
        } else {
            0
        }
    }
    /// Drop seats of players who left once no match needs them.
    fn purge_left(&mut self) {
        if self.game.is_none() {
            self.players.retain(|m| !m.left);
        }
    }
    /// Why `m` can't ready up with its teammate's deck, if any.
    fn team_conflict(&self, m: &Member) -> Option<String> {
        if self.settings.mode != Mode::Teams {
            return None;
        }
        let legend = m.deck.as_ref()?.legend.as_ref()?;
        let clash = self.players.iter().any(|o| o.id != m.id && o.team == m.team && o.deck.as_ref().and_then(|d| d.legend.as_ref()) == Some(legend));
        clash.then(|| "Your teammate uses the same legend. Teammates need different legends.".to_string())
    }
}

struct QueueEntry {
    id: u64,
    mode: QueueMode,
    deck: Deck,
    since: Instant,
}

pub struct Hub {
    db: Arc<CardDb>,
    clients: HashMap<u64, Client>,
    tokens: HashMap<String, u64>,
    users: HashMap<i64, u64>,
    rooms: HashMap<String, Room>,
    queue: Vec<QueueEntry>,
    next_id: u64,
    next_conn: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("os randomness");
    u64::from_le_bytes(b)
}

fn random_token() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("os randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Hub {
    pub fn new(db: Arc<CardDb>) -> Hub {
        Hub { db, clients: HashMap::new(), tokens: HashMap::new(), users: HashMap::new(), rooms: HashMap::new(), queue: vec![], next_id: 1, next_conn: 1 }
    }

    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    fn send(&self, id: u64, msg: ServerMsg) {
        if let Some(tx) = self.clients.get(&id).and_then(|c| c.tx.as_ref()) {
            let _ = tx.send(msg);
        }
    }
    fn err(&self, id: u64, m: impl Into<String>) {
        self.send(id, ServerMsg::Error { message: m.into() });
    }
    fn name(&self, id: u64) -> String {
        self.clients.get(&id).map(|c| c.name.clone()).unwrap_or_else(|| "Player".into())
    }

    /// Register a connection. A signed-in user always maps to the same hub
    /// client (so a refresh or new tab keeps the seat); guests can resume with
    /// their token. Returns (client id, connection id).
    pub fn hello(&mut self, identity: Option<Identity>, token: Option<&str>, tx: Tx) -> (u64, u64) {
        let conn = self.next_conn;
        self.next_conn += 1;
        let existing = match &identity {
            Some(u) => self.users.get(&u.user_id).copied(),
            None => token.and_then(|t| self.tokens.get(t).copied()).filter(|id| self.clients.get(id).map(|c| c.user.is_none()).unwrap_or(false)),
        };
        let id = match existing {
            Some(id) => {
                let c = self.clients.get_mut(&id).unwrap();
                if let Some(old) = c.tx.take() {
                    let _ = old.send(ServerMsg::Replaced);
                }
                c.tx = Some(tx);
                c.conn = conn;
                if let Some(u) = &identity {
                    c.name = u.name.clone();
                }
                c.last_seen = Instant::now();
                id
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                let token = random_token();
                self.tokens.insert(token.clone(), id);
                let (name, user) = match &identity {
                    Some(u) => (u.name.clone(), Some(u.user_id)),
                    None => ("Guest".to_string(), None),
                };
                if let Some(uid) = user {
                    self.users.insert(uid, id);
                }
                self.clients.insert(id, Client { name, user, token, tx: Some(tx), conn, room: None, last_seen: Instant::now() });
                id
            }
        };
        let c = &self.clients[&id];
        self.send(id, ServerMsg::Welcome { id, token: c.token.clone(), name: c.name.clone(), signed_in: c.user.is_some() });
        if let Some(code) = self.clients[&id].room.clone() {
            if let Some(r) = self.rooms.get(&code) {
                self.send(id, ServerMsg::ChatHistory { lines: r.chat.clone() });
            }
            self.broadcast_room(&code);
        }
        (id, conn)
    }

    /// Socket closed. Ignored if the client already reconnected on a newer connection.
    pub fn disconnect(&mut self, id: u64, conn: u64) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        if c.conn != conn {
            return;
        }
        c.tx = None;
        c.last_seen = Instant::now();
        self.queue.retain(|q| q.id != id);
        if let Some(code) = c.room.clone() {
            self.broadcast_room(&code);
        }
    }

    /// Drop sessions and rooms abandoned for longer than the TTL.
    pub fn sweep(&mut self) {
        let now = Instant::now();
        let stale: Vec<u64> = self.clients.iter().filter(|(_, c)| c.tx.is_none() && now.duration_since(c.last_seen) > SESSION_TTL).map(|(id, _)| *id).collect();
        for id in stale {
            self.leave_room(id, true);
            if let Some(c) = self.clients.remove(&id) {
                self.tokens.remove(&c.token);
                if let Some(u) = c.user {
                    self.users.remove(&u);
                }
            }
        }
    }

    pub fn handle(&mut self, id: u64, msg: ClientMsg) {
        if let Some(c) = self.clients.get_mut(&id) {
            c.last_seen = Instant::now();
        }
        let guest = self.clients.get(&id).map(|c| c.user.is_none()).unwrap_or(true);
        let guest_ok = matches!(msg, ClientMsg::Hello { .. } | ClientMsg::Ping | ClientMsg::JoinRoom { spectate: true, .. } | ClientMsg::LeaveRoom | ClientMsg::ListRooms | ClientMsg::CancelQueue);
        if guest && !guest_ok {
            self.send(id, ServerMsg::SignInRequired { message: "Sign in to play online. Guests can only watch.".into() });
            return;
        }
        match msg {
            ClientMsg::Hello { .. } => {}
            ClientMsg::Ping => self.send(id, ServerMsg::Pong),
            ClientMsg::CreateRoom { settings } => {
                self.create_room(id, sanitize(settings), false);
            }
            ClientMsg::JoinRoom { code, spectate } => self.join_room(id, &code, spectate),
            ClientMsg::LeaveRoom => {
                self.leave_room(id, false);
                self.send(id, ServerMsg::LeftRoom);
            }
            ClientMsg::UpdateSettings { settings } => self.update_settings(id, sanitize(settings)),
            ClientMsg::SetDeck { deck, sealed } => self.set_deck(id, deck, sealed),
            ClientMsg::SetReady { ready } => self.set_ready(id, ready),
            ClientMsg::ToggleSpectator => self.toggle_spectator(id),
            ClientMsg::SetTeam { team } => self.set_team(id, team),
            ClientMsg::Act { action } => self.act(id, action),
            ClientMsg::ReturnToLobby => self.return_to_lobby(id),
            ClientMsg::Chat { text } => self.chat(id, &text),
            ClientMsg::ListRooms => self.list_rooms(id),
            ClientMsg::Queue { mode, deck, sealed: _ } => self.enqueue(id, mode, deck),
            ClientMsg::CancelQueue => {
                self.queue.retain(|q| q.id != id);
                self.send(id, ServerMsg::Queue { status: QueueStatus { searching: false, mode: None, waited_secs: 0, in_queue: self.queue.len() } });
            }
        }
    }

    /// The account's name changed (account settings). Rooms show it at once;
    /// a match in progress keeps the names it started with.
    pub fn rename_user(&mut self, user_id: i64, name: &str) {
        let Some(&id) = self.users.get(&user_id) else { return };
        if let Some(c) = self.clients.get_mut(&id) {
            c.name = name.to_string();
        }
        self.send(id, ServerMsg::Welcome { id, token: self.clients[&id].token.clone(), name: name.to_string(), signed_in: true });
        if let Some(code) = self.room_of(id) {
            self.broadcast_room(&code);
        }
    }

    /// The account's sessions were revoked (password reset). Its open socket
    /// reconnects without the session; the seat is kept for a later sign-in.
    pub fn sign_out_user(&mut self, user_id: i64) {
        if let Some(&id) = self.users.get(&user_id) {
            self.send(id, ServerMsg::SignedOut);
        }
    }

    fn room_of(&self, id: u64) -> Option<String> {
        self.clients.get(&id).and_then(|c| c.room.clone())
    }

    fn new_code(&mut self) -> String {
        loop {
            let mut x = random_u64();
            let code: String = (0..6)
                .map(|_| {
                    let c = CODE_ALPHABET[(x % CODE_ALPHABET.len() as u64) as usize] as char;
                    x /= CODE_ALPHABET.len() as u64;
                    c
                })
                .collect();
            if !self.rooms.contains_key(&code) {
                return code;
            }
        }
    }

    fn create_room(&mut self, id: u64, settings: RoomSettings, from_queue: bool) -> String {
        self.leave_room(id, false);
        self.queue.retain(|q| q.id != id);
        let code = self.new_code();
        let room = Room { code: code.clone(), settings, host: id, players: vec![Member::new(id, 0)], spectators: vec![], game: None, chat: vec![], from_queue };
        self.rooms.insert(code.clone(), room);
        if let Some(c) = self.clients.get_mut(&id) {
            c.room = Some(code.clone());
        }
        self.system_chat(&code, &format!("{} created the room.", self.name(id)));
        self.broadcast_room(&code);
        code
    }

    fn join_room(&mut self, id: u64, code: &str, spectate: bool) {
        let code = code.trim().to_ascii_uppercase();
        if self.room_of(id).as_deref() == Some(code.as_str()) {
            self.broadcast_room(&code);
            return;
        }
        let Some(room) = self.rooms.get(&code) else {
            return self.err(id, "Room not found. Check the room code.");
        };
        let as_player = !spectate && room.players.len() < room.seats() && room.game.is_none();
        if !as_player {
            if !room.settings.allow_spectators {
                return self.err(id, if spectate { "Spectators are disabled for this room." } else { "That room is full." });
            }
            if room.spectators.len() >= MAX_SPECTATORS {
                return self.err(id, "Too many spectators in that room.");
            }
        }
        self.leave_room(id, false);
        self.queue.retain(|q| q.id != id);
        let room = self.rooms.get_mut(&code).unwrap();
        if as_player {
            let team = room.open_team();
            room.players.push(Member::new(id, team));
        } else {
            room.spectators.push(id);
        }
        self.clients.get_mut(&id).unwrap().room = Some(code.clone());
        let lines = self.rooms[&code].chat.clone();
        self.send(id, ServerMsg::ChatHistory { lines });
        let what = if as_player { "joined the room" } else { "is spectating" };
        self.system_chat(&code, &format!("{} {what}.", self.name(id)));
        self.broadcast_room(&code);
    }

    /// Remove a client from its room. During a match a leaving player concedes.
    fn leave_room(&mut self, id: u64, silent: bool) {
        let Some(code) = self.room_of(id) else { return };
        if let Some(c) = self.clients.get_mut(&id) {
            c.room = None;
        }
        let name = self.name(id);
        let db = self.db.clone();
        let Some(room) = self.rooms.get_mut(&code) else { return };
        let was_player = room.seat_of(id);
        room.spectators.retain(|&s| s != id);
        if let Some(seat) = was_player {
            if let Some(m) = room.game.as_mut() {
                let _ = m.apply(&db, seat as u8, rift_shared::session::MatchAction::Game(rift_shared::game::Action::Concede));
            }
            // In a free-for-all the others play on: keep the seat until the match ends.
            let keep = room.game.as_ref().map(|m| m.game.stage != rift_shared::game::Stage::Over && m.seats() > 2).unwrap_or(false);
            if keep {
                room.players[seat].left = true;
                room.players[seat].ready = false;
            } else {
                room.game = None;
                room.players.remove(seat);
                room.purge_left();
                for p in room.players.iter_mut() {
                    p.ready = false;
                }
            }
        }
        if room.active_players() == 0 && room.spectators.is_empty() {
            self.rooms.remove(&code);
            return;
        }
        if room.host == id {
            room.host = room.players.iter().find(|m| !m.left).map(|m| m.id).or(room.spectators.first().copied()).unwrap_or(0);
        }
        if !silent {
            self.system_chat(&code, &format!("{name} left the room."));
        }
        self.broadcast_room(&code);
    }

    fn update_settings(&mut self, id: u64, settings: RoomSettings) {
        let Some(code) = self.room_of(id) else { return self.err(id, "You are not in a room.") };
        let db = self.db.clone();
        let room = self.rooms.get_mut(&code).unwrap();
        if room.host != id {
            return self.err(id, "Only the host can change room settings.");
        }
        if room.game.is_some() {
            return self.err(id, "Settings can't change during a match.");
        }
        if room.players.len() > settings.mode.seats() {
            return self.err(id, format!("{} seats {} players. Move someone to spectators first.", settings.mode.label(), settings.mode.seats()));
        }
        let rules_changed = room.settings.rules != settings.rules;
        let mode_changed = room.settings.mode != settings.mode;
        room.settings = settings;
        if mode_changed {
            for (i, m) in room.players.iter_mut().enumerate() {
                m.team = if room.settings.mode == Mode::Teams { (i % 2) as u8 } else { 0 };
                m.ready = false;
            }
        }
        if rules_changed {
            let rules = room.settings.rules;
            for m in room.players.iter_mut() {
                m.deck_errors = deck_errors(&db, m.deck.as_ref(), m.sealed.as_ref(), rules);
                m.ready = false;
            }
        }
        self.broadcast_room(&code);
    }

    fn set_deck(&mut self, id: u64, deck: Deck, sealed: Option<SealedRef>) {
        let Some(code) = self.room_of(id) else { return self.err(id, "Join a room before choosing a deck.") };
        let db = self.db.clone();
        let room = self.rooms.get_mut(&code).unwrap();
        if room.game.is_some() {
            return self.err(id, "You can't change decks during a match.");
        }
        let rules = room.settings.rules;
        let Some(m) = room.players.iter_mut().find(|m| m.id == id) else { return self.err(id, "Spectators don't need a deck.") };
        m.deck_errors = deck_errors(&db, Some(&deck), sealed.as_ref(), rules);
        m.deck = Some(deck);
        m.sealed = sealed;
        m.ready = false;
        self.broadcast_room(&code);
    }

    fn set_ready(&mut self, id: u64, ready: bool) {
        let Some(code) = self.room_of(id) else { return };
        let room = self.rooms.get_mut(&code).unwrap();
        if room.game.is_some() {
            return;
        }
        let Some(m) = room.players.iter_mut().find(|m| m.id == id) else { return };
        if ready && (m.deck.is_none() || !m.deck_errors.is_empty()) {
            return self.err(id, "Load a valid deck before readying up.");
        }
        let i = room.players.iter().position(|m| m.id == id).unwrap();
        if ready {
            if let Some(e) = room.team_conflict(&room.players[i]) {
                return self.err(id, e);
            }
        }
        room.players[i].ready = ready;
        let teams_full = room.settings.mode != Mode::Teams || room.players.iter().filter(|m| m.team == 0).count() == 2;
        let start = room.players.len() == room.seats() && teams_full && room.players.iter().all(|m| m.ready);
        if start {
            self.start_match(&code);
        } else {
            self.broadcast_room(&code);
        }
    }

    fn start_match(&mut self, code: &str) {
        let seed = random_u64();
        let db = self.db.clone();
        let room = self.rooms.get_mut(code).unwrap();
        if room.settings.mode == Mode::Teams {
            // Seat order A, B, A, B so turns alternate between the teams.
            let (mut a, mut b): (Vec<Member>, Vec<Member>) = room.players.drain(..).partition(|m| m.team == 0);
            while !a.is_empty() || !b.is_empty() {
                if !a.is_empty() {
                    room.players.push(a.remove(0));
                }
                if !b.is_empty() {
                    room.players.push(b.remove(0));
                }
            }
        }
        let names: Vec<String> = self.rooms[code].players.iter().map(|m| self.name(m.id)).collect();
        let room = self.rooms.get_mut(code).unwrap();
        let s = &room.settings;
        let cfg = MatchConfig { format: s.format, rules: s.rules, solo: false, auto_steps: s.auto_steps, win_points: s.win_points, mode: s.mode };
        let decks: Vec<Deck> = room.players.iter().map(|m| m.deck.clone().unwrap_or_default()).collect();
        room.game = Some(Match::new(&db, cfg, names, decks, seed));
        self.system_chat(code, "Started the match.");
        self.broadcast_room(code);
    }

    fn toggle_spectator(&mut self, id: u64) {
        let Some(code) = self.room_of(id) else { return };
        let room = self.rooms.get_mut(&code).unwrap();
        if room.game.is_some() {
            return self.err(id, "You can't switch roles during a match.");
        }
        if let Some(i) = room.players.iter().position(|m| m.id == id) {
            if !room.settings.allow_spectators {
                return self.err(id, "Spectators are disabled for this room.");
            }
            room.players.remove(i);
            room.spectators.push(id);
        } else if room.players.len() < room.seats() {
            room.spectators.retain(|&s| s != id);
            let team = room.open_team();
            room.players.push(Member::new(id, team));
        } else {
            return self.err(id, "All player seats are taken.");
        }
        self.broadcast_room(&code);
    }

    fn set_team(&mut self, id: u64, team: u8) {
        let Some(code) = self.room_of(id) else { return };
        let room = self.rooms.get_mut(&code).unwrap();
        if room.settings.mode != Mode::Teams || team > 1 {
            return self.err(id, "Teams are only used in 2v2 rooms.");
        }
        if room.game.is_some() {
            return self.err(id, "You can't switch teams during a match.");
        }
        let Some(i) = room.players.iter().position(|m| m.id == id) else { return self.err(id, "Take a player seat first.") };
        if room.players[i].team == team {
            return;
        }
        if room.players.iter().filter(|m| m.team == team).count() >= 2 {
            return self.err(id, format!("{} is full.", rift_shared::game::team_name(team)));
        }
        room.players[i].team = team;
        for m in room.players.iter_mut() {
            m.ready = false;
        }
        self.broadcast_room(&code);
    }

    fn act(&mut self, id: u64, action: rift_shared::session::MatchAction) {
        let Some(code) = self.room_of(id) else { return self.err(id, "You are not in a room.") };
        let db = self.db.clone();
        let room = self.rooms.get_mut(&code).unwrap();
        let Some(seat) = room.seat_of(id) else { return self.err(id, "Spectators can't take game actions.") };
        let Some(m) = room.game.as_mut() else { return self.err(id, "The match has not started.") };
        match m.apply(&db, seat as u8, action) {
            Ok(()) => self.broadcast_match(&code),
            Err(e) => self.err(id, e),
        }
    }

    fn return_to_lobby(&mut self, id: u64) {
        let Some(code) = self.room_of(id) else { return };
        let room = self.rooms.get_mut(&code).unwrap();
        if room.seat_of(id).is_none() && room.host != id {
            return self.err(id, "Only players can end the match.");
        }
        if room.game.is_none() {
            return;
        }
        room.game = None;
        room.purge_left();
        for p in room.players.iter_mut() {
            p.ready = false;
        }
        self.system_chat(&code, &format!("{} returned the room to the lobby.", self.name(id)));
        self.broadcast_room(&code);
    }

    fn chat(&mut self, id: u64, text: &str) {
        let Some(code) = self.room_of(id) else { return self.err(id, "Join a room to chat.") };
        let Some(text) = clean_chat(text) else { return };
        let line = ChatLine { from: self.name(id), text, system: false, ts: now_ms() };
        self.push_chat(&code, line);
    }

    fn system_chat(&mut self, code: &str, text: &str) {
        let line = ChatLine { from: String::new(), text: text.to_string(), system: true, ts: now_ms() };
        self.push_chat(code, line);
    }

    fn push_chat(&mut self, code: &str, line: ChatLine) {
        let Some(room) = self.rooms.get_mut(code) else { return };
        room.chat.push(line.clone());
        if room.chat.len() > CHAT_HISTORY {
            room.chat.remove(0);
        }
        for m in members(room) {
            self.send(m, ServerMsg::Chat { line: line.clone() });
        }
    }

    fn list_rooms(&self, id: u64) {
        let mut rooms: Vec<RoomSummary> = self
            .rooms
            .values()
            .filter(|r| r.settings.public || (r.game.is_some() && r.settings.allow_spectators && r.from_queue))
            .map(|r| RoomSummary { code: r.code.clone(), host_name: self.name(r.host), players: r.active_players(), spectators: r.spectators.len(), in_game: r.game.is_some(), settings: r.settings.clone() })
            .collect();
        rooms.sort_by(|a, b| a.in_game.cmp(&b.in_game).then(a.code.cmp(&b.code)));
        self.send(id, ServerMsg::Rooms { rooms });
    }

    fn enqueue(&mut self, id: u64, mode: QueueMode, deck: Deck) {
        if mode.rules == DeckRules::Sealed {
            return self.err(id, "Sealed formats are not available in public matchmaking.");
        }
        let errs = deck.validate(&self.db, mode.rules);
        if !errs.is_empty() {
            return self.err(id, format!("Your deck is not legal for this queue: {}", errs.join(" ")));
        }
        self.leave_room(id, false);
        self.queue.retain(|q| q.id != id);
        let opponent = self.queue.iter().position(|q| q.mode == mode && q.id != id && self.clients.get(&q.id).map(|c| c.tx.is_some()).unwrap_or(false));
        match opponent {
            Some(i) => {
                let other = self.queue.remove(i);
                let settings = RoomSettings { format: mode.format, rules: mode.rules, public: false, allow_spectators: true, ..Default::default() };
                let code = self.create_room(other.id, settings, true);
                self.join_room(id, &code, false);
                let db = self.db.clone();
                let room = self.rooms.get_mut(&code).unwrap();
                for m in room.players.iter_mut() {
                    let d = if m.id == id { deck.clone() } else { other.deck.clone() };
                    m.deck_errors = deck_errors(&db, Some(&d), None, mode.rules);
                    m.deck = Some(d);
                    m.ready = true;
                }
                for p in [other.id, id] {
                    self.send(p, ServerMsg::Queue { status: QueueStatus { searching: false, mode: Some(mode.clone()), waited_secs: 0, in_queue: self.queue.len() } });
                    self.send(p, ServerMsg::MatchFound { code: code.clone() });
                }
                self.start_match(&code);
            }
            None => {
                self.queue.push(QueueEntry { id, mode: mode.clone(), deck, since: Instant::now() });
                let n = self.queue.iter().filter(|q| q.mode == mode).count();
                self.send(id, ServerMsg::Queue { status: QueueStatus { searching: true, mode: Some(mode), waited_secs: 0, in_queue: n } });
            }
        }
    }

    /// Periodic queue status for waiting players.
    pub fn queue_tick(&self) {
        for q in &self.queue {
            let n = self.queue.iter().filter(|o| o.mode == q.mode).count();
            self.send(q.id, ServerMsg::Queue { status: QueueStatus { searching: true, mode: Some(q.mode.clone()), waited_secs: q.since.elapsed().as_secs(), in_queue: n } });
        }
    }

    fn member_view(&self, m: &Member, room: Option<&Room>) -> MemberView {
        let c = self.clients.get(&m.id);
        let mut deck_errors = m.deck_errors.clone();
        if let Some(e) = room.and_then(|r| r.team_conflict(m)) {
            deck_errors.push(e);
        }
        MemberView {
            id: m.id,
            name: self.name(m.id),
            connected: c.map(|c| c.tx.is_some()).unwrap_or(false),
            ready: m.ready,
            deck_name: m.deck.as_ref().map(|d| if d.name.is_empty() { "Unnamed deck".into() } else { d.name.clone() }),
            legend: m.deck.as_ref().and_then(|d| d.legend.clone()),
            deck_ok: m.deck.is_some() && deck_errors.is_empty(),
            deck_errors,
            team: m.team,
            left: m.left,
        }
    }

    fn broadcast_room(&self, code: &str) {
        let Some(room) = self.rooms.get(code) else { return };
        let players: Vec<MemberView> = room.players.iter().map(|m| self.member_view(m, Some(room))).collect();
        let spectators: Vec<MemberView> = room
            .spectators
            .iter()
            .map(|&s| self.member_view(&Member::new(s, 0), None))
            .collect();
        for id in members(room) {
            let you = match room.seat_of(id) {
                Some(i) => Role::Player(i as u8),
                None => Role::Spectator,
            };
            let view = RoomView { code: room.code.clone(), settings: room.settings.clone(), host: room.host, players: players.clone(), spectators: spectators.clone(), in_game: room.game.is_some(), from_queue: room.from_queue, you };
            self.send(id, ServerMsg::Room { room: view });
        }
        self.broadcast_match(code);
    }

    fn broadcast_match(&self, code: &str) {
        let Some(room) = self.rooms.get(code) else { return };
        let Some(m) = room.game.as_ref() else { return };
        for id in members(room) {
            let seat = room.seat_of(id).map(|s| s as u8);
            let view = m.view(seat, room.settings.spectators_see_hands);
            self.send(id, ServerMsg::Match { view: Box::new(view) });
        }
    }
}

fn members(room: &Room) -> Vec<u64> {
    room.players.iter().filter(|m| !m.left).map(|m| m.id).chain(room.spectators.iter().copied()).collect()
}

fn sanitize(mut s: RoomSettings) -> RoomSettings {
    s.win_points = s.win_points.clamp(1, 30);
    if s.mode != Mode::Duel {
        s.format = rift_shared::session::Format::Bo1;
    }
    s
}

fn deck_errors(db: &CardDb, deck: Option<&Deck>, sealed: Option<&SealedRef>, rules: DeckRules) -> Vec<String> {
    let Some(deck) = deck else { return vec![] };
    let mut e = deck.validate(db, rules);
    if rules == DeckRules::Sealed {
        match sealed {
            None => e.push("Sealed rooms need a deck built from a sealed pool.".into()),
            Some(s) => {
                if s.sets.is_empty() || s.sets.len() > 12 || s.seed.trim().is_empty() {
                    e.push("That sealed pool is not valid.".into());
                } else {
                    let pool = sealed::open_pool(db, &s.sets, &s.seed);
                    e.extend(sealed::check_pool(db, deck, &pool));
                }
            }
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use rift_shared::decklist::starter_deck;
    use rift_shared::game::{Action, Stage};
    use rift_shared::session::{Format, MatchAction};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn db() -> Arc<CardDb> {
        Arc::new(CardDb::from_json(include_str!("../../data/cards.json")).unwrap())
    }

    struct C {
        id: u64,
        conn: u64,
        rx: UnboundedReceiver<ServerMsg>,
    }
    impl C {
        fn drain(&mut self) -> Vec<ServerMsg> {
            let mut v = vec![];
            while let Ok(m) = self.rx.try_recv() {
                v.push(m);
            }
            v
        }
        fn last_room(&mut self) -> Option<RoomView> {
            self.drain().into_iter().rev().find_map(|m| if let ServerMsg::Room { room } = m { Some(room) } else { None })
        }
    }

    fn connect(h: &mut Hub, name: &str) -> C {
        let (tx, rx) = unbounded_channel();
        let user_id = rift_shared::rng::seed_from_str(name) as i64 & 0xffff_ffff;
        let (id, conn) = h.hello(Some(Identity { user_id, name: name.into() }), None, tx);
        C { id, conn, rx }
    }

    fn token_of(c: &mut C) -> String {
        c.drain().into_iter().find_map(|m| if let ServerMsg::Welcome { token, .. } = m { Some(token) } else { None }).unwrap()
    }

    #[test]
    fn room_lifecycle_and_match() {
        let db = db();
        let mut h = Hub::new(db.clone());
        let mut a = connect(&mut h, "Alice");
        let mut b = connect(&mut h, "Bob");
        h.handle(a.id, ClientMsg::CreateRoom { settings: RoomSettings::default() });
        let room = a.last_room().unwrap();
        assert_eq!(room.code.len(), 6);
        assert_eq!(room.you, Role::Player(0));
        h.handle(b.id, ClientMsg::JoinRoom { code: room.code.to_lowercase(), spectate: false });
        let rb = b.last_room().unwrap();
        assert_eq!(rb.players.len(), 2);
        assert_eq!(rb.you, Role::Player(1));
        // Not ready without a valid deck
        h.handle(a.id, ClientMsg::SetReady { ready: true });
        assert!(a.drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
        let mut bad = starter_deck(&db, "SFD-181").unwrap();
        bad.runes.clear();
        h.handle(a.id, ClientMsg::SetDeck { deck: bad, sealed: None });
        assert!(!a.last_room().unwrap().players[0].deck_ok);
        h.handle(a.id, ClientMsg::SetDeck { deck: starter_deck(&db, "SFD-181").unwrap(), sealed: None });
        h.handle(b.id, ClientMsg::SetDeck { deck: starter_deck(&db, "SFD-205").unwrap(), sealed: None });
        h.handle(a.id, ClientMsg::SetReady { ready: true });
        h.handle(b.id, ClientMsg::SetReady { ready: true });
        let msgs = a.drain();
        let view = msgs.iter().rev().find_map(|m| if let ServerMsg::Match { view } = m { Some(view.clone()) } else { None }).expect("match started");
        assert_eq!(view.game.stage, Stage::ChooseBattlefield);
        // Opponent's deck/hand hidden
        assert!(view.game.cards.iter().filter(|c| c.owner == 1 && matches!(c.zone, rift_shared::game::Zone::Deck(_))).all(|c| c.code.is_empty()));
        // Spectator joins and sees match
        let mut s = connect(&mut h, "Sam");
        h.handle(s.id, ClientMsg::JoinRoom { code: room.code.clone(), spectate: false });
        let sm = s.drain();
        assert!(sm.iter().any(|m| matches!(m, ServerMsg::Room { room } if room.you == Role::Spectator)));
        assert!(sm.iter().any(|m| matches!(m, ServerMsg::Match { .. })));
        h.handle(s.id, ClientMsg::Act { action: MatchAction::Game(Action::Concede) });
        assert!(s.drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
        // A game action by a player is broadcast
        let bf = view.game.seats[0].bf_options[0].clone();
        h.handle(a.id, ClientMsg::Act { action: MatchAction::Game(Action::ChooseBattlefield { code: bf }) });
        assert!(b.drain().iter().any(|m| matches!(m, ServerMsg::Match { .. })));
        // chat reaches everyone
        h.handle(b.id, ClientMsg::Chat { text: "glhf".into() });
        assert!(s.drain().iter().any(|m| matches!(m, ServerMsg::Chat { line } if line.text == "glhf" && line.from == "Bob")));
        // leaving mid-match concedes and resets the room
        h.handle(b.id, ClientMsg::LeaveRoom);
        let r = a.last_room().unwrap();
        assert!(!r.in_game);
        assert_eq!(r.players.len(), 1);
        h.handle(a.id, ClientMsg::LeaveRoom);
        h.handle(s.id, ClientMsg::LeaveRoom);
        assert_eq!(h.room_count(), 0);
    }

    fn last_match(c: &mut C) -> Option<Box<rift_shared::session::MatchView>> {
        c.drain().into_iter().rev().find_map(|m| if let ServerMsg::Match { view } = m { Some(view) } else { None })
    }

    /// Four signed-in players in one room with the given mode, each with a different starter deck.
    fn four_room(h: &mut Hub, db: &CardDb, mode: Mode) -> (Vec<C>, String) {
        let mut cs: Vec<C> = ["Ann", "Bo", "Cy", "Di"].iter().map(|n| connect(h, n)).collect();
        h.handle(cs[0].id, ClientMsg::CreateRoom { settings: RoomSettings { mode, win_points: mode.default_points(), ..Default::default() } });
        let code = cs[0].last_room().unwrap().code;
        for c in &cs[1..] {
            h.handle(c.id, ClientMsg::JoinRoom { code: code.clone(), spectate: false });
        }
        for (c, l) in cs.iter().zip(["SFD-181", "SFD-205", "UNL-193", "UNL-187"]) {
            h.handle(c.id, ClientMsg::SetDeck { deck: starter_deck(db, l).unwrap(), sealed: None });
        }
        for c in cs.iter_mut() {
            c.drain();
        }
        (cs, code)
    }

    #[test]
    fn teams_room_seats_four_and_alternates_teams() {
        let db = db();
        let mut h = Hub::new(db.clone());
        let (mut cs, code) = four_room(&mut h, &db, Mode::Teams);
        // Joiners were balanced A, B, A, B; put Ann and Bo together instead.
        h.handle(cs[2].id, ClientMsg::SetTeam { team: 1 });
        assert!(cs[2].drain().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("full"))));
        h.handle(cs[1].id, ClientMsg::SetTeam { team: 0 });
        assert!(cs[1].drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })), "team A already has two");
        h.handle(cs[2].id, ClientMsg::ToggleSpectator);
        h.handle(cs[1].id, ClientMsg::SetTeam { team: 0 });
        h.handle(cs[2].id, ClientMsg::ToggleSpectator);
        let r = cs[0].last_room().unwrap();
        assert_eq!(r.players.iter().map(|p| (p.name.as_str(), p.team)).collect::<Vec<_>>(), vec![("Ann", 0), ("Bo", 0), ("Di", 1), ("Cy", 1)]);
        // A fifth player can only watch.
        let mut e = connect(&mut h, "Eve");
        h.handle(e.id, ClientMsg::JoinRoom { code: code.clone(), spectate: false });
        assert_eq!(e.last_room().unwrap().you, Role::Spectator);
        // Teammates may not share a legend.
        h.handle(cs[2].id, ClientMsg::SetDeck { deck: starter_deck(&db, "UNL-187").unwrap(), sealed: None });
        assert!(cs[3].last_room().unwrap().players[3].deck_errors.iter().any(|e| e.contains("same legend")));
        h.handle(cs[2].id, ClientMsg::SetReady { ready: true });
        assert!(cs[2].drain().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("same legend"))));
        h.handle(cs[2].id, ClientMsg::SetDeck { deck: starter_deck(&db, "UNL-193").unwrap(), sealed: None });
        // Bo1 is forced, and the mode can't shrink below the player count.
        h.handle(cs[0].id, ClientMsg::UpdateSettings { settings: RoomSettings { mode: Mode::Teams, format: rift_shared::session::Format::Bo3, win_points: 11, ..Default::default() } });
        assert_eq!(cs[0].last_room().unwrap().settings.format, rift_shared::session::Format::Bo1);
        h.handle(cs[0].id, ClientMsg::UpdateSettings { settings: RoomSettings::default() });
        assert!(cs[0].drain().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("seats 2"))));
        for c in &cs {
            h.handle(c.id, ClientMsg::SetReady { ready: true });
        }
        // Seats are reordered A, B, A, B: Ann, Di, Bo, Cy.
        let r = cs[0].last_room().unwrap();
        assert!(r.in_game);
        assert_eq!(r.players.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Ann", "Di", "Bo", "Cy"]);
        let v = last_match(&mut cs[1]).unwrap();
        assert_eq!(v.viewer, Some(2));
        assert_eq!(v.config.mode, Mode::Teams);
        assert_eq!(v.game.options.win_points, 11);
        assert!(matches!(v.game.stage, Stage::ChooseOrder { .. }));
        // Bo (seat 2) sees teammate Ann's hand once hands are dealt; Di does not.
        let Stage::ChooseOrder { chooser } = v.game.stage else { unreachable!() };
        let seat_client = |s: u8| ["Ann", "Di", "Bo", "Cy"][s as usize];
        let idx = |n: &str| ["Ann", "Bo", "Cy", "Di"].iter().position(|x| *x == n).unwrap();
        h.handle(cs[idx(seat_client(chooser))].id, ClientMsg::Act { action: MatchAction::Game(Action::ChooseOrder { go_first: true }) });
        for s in 0..4u8 {
            if s == chooser {
                continue;
            }
            let c = idx(seat_client(s));
            let v = last_match(&mut cs[c]).unwrap();
            let b = v.game.seats[s as usize].bf_options[0].clone();
            h.handle(cs[c].id, ClientMsg::Act { action: MatchAction::Game(Action::ChooseBattlefield { code: b }) });
        }
        let bo = last_match(&mut cs[1]).unwrap();
        assert_eq!(bo.game.stage, Stage::Mulligan);
        assert!(bo.game.hand(0).iter().all(|&id| !bo.game.cards[id as usize].code.is_empty()));
        let di = last_match(&mut cs[3]).unwrap();
        assert!(di.game.hand(0).iter().all(|&id| di.game.cards[id as usize].code.is_empty()));
        // Leaving concedes for the team and ends the match for everyone.
        h.handle(cs[3].id, ClientMsg::LeaveRoom);
        let r = cs[0].last_room().unwrap();
        assert!(!r.in_game);
        assert_eq!(r.players.len(), 3);
    }

    #[test]
    fn three_player_room() {
        let db = db();
        let mut h = Hub::new(db.clone());
        let (mut cs, code) = four_room(&mut h, &db, Mode::FreeForAll);
        // Four players can't switch to a three-seat room until one steps out.
        h.handle(cs[0].id, ClientMsg::UpdateSettings { settings: RoomSettings { mode: Mode::Skirmish, ..Default::default() } });
        assert!(cs[0].drain().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("seats 3"))));
        h.handle(cs[3].id, ClientMsg::ToggleSpectator);
        h.handle(cs[0].id, ClientMsg::UpdateSettings { settings: RoomSettings { mode: Mode::Skirmish, ..Default::default() } });
        assert_eq!(cs[0].last_room().unwrap().settings.mode, Mode::Skirmish);
        // The spectator can't take a fourth seat.
        h.handle(cs[3].id, ClientMsg::ToggleSpectator);
        assert!(cs[3].drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
        for c in &cs[..3] {
            h.handle(c.id, ClientMsg::SetReady { ready: true });
        }
        let r = cs[0].last_room().unwrap();
        assert!(r.in_game, "{code}");
        let v = last_match(&mut cs[1]).unwrap();
        assert_eq!(v.game.seats.len(), 3);
        assert_eq!(v.config.mode, Mode::Skirmish);
    }

    #[test]
    fn free_for_all_goes_on_when_a_player_leaves() {
        let db = db();
        let mut h = Hub::new(db.clone());
        let (mut cs, code) = four_room(&mut h, &db, Mode::FreeForAll);
        for c in &cs {
            h.handle(c.id, ClientMsg::SetReady { ready: true });
        }
        assert!(cs[0].last_room().unwrap().in_game);
        h.handle(cs[2].id, ClientMsg::LeaveRoom);
        let r = cs[0].last_room().unwrap();
        assert!(r.in_game, "three players remain");
        assert!(r.players[2].left);
        assert_eq!(r.you, Role::Player(0));
        let v = last_match(&mut cs[3]).unwrap();
        assert_eq!(v.viewer, Some(3));
        assert!(v.game.seats[2].conceded);
        // The departed player can come back only as a spectator and can't act for the old seat.
        h.handle(cs[2].id, ClientMsg::JoinRoom { code: code.clone(), spectate: false });
        assert_eq!(cs[2].last_room().unwrap().you, Role::Spectator);
        h.handle(cs[2].id, ClientMsg::Act { action: MatchAction::Game(Action::RollD20) });
        assert!(cs[2].drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
        // Back to the lobby frees the seat.
        h.handle(cs[0].id, ClientMsg::ReturnToLobby);
        let r = cs[0].last_room().unwrap();
        assert!(!r.in_game);
        assert_eq!(r.players.len(), 3);
        assert!(r.players.iter().all(|p| !p.left));
    }

    #[test]
    fn host_only_settings_and_spectator_toggle() {
        let db = db();
        let mut h = Hub::new(db);
        let mut a = connect(&mut h, "A");
        let mut b = connect(&mut h, "B");
        h.handle(a.id, ClientMsg::CreateRoom { settings: RoomSettings::default() });
        let code = a.last_room().unwrap().code;
        h.handle(b.id, ClientMsg::JoinRoom { code: code.clone(), spectate: true });
        assert_eq!(b.last_room().unwrap().you, Role::Spectator);
        h.handle(b.id, ClientMsg::UpdateSettings { settings: RoomSettings { format: Format::Bo3, ..Default::default() } });
        assert!(b.drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
        h.handle(a.id, ClientMsg::UpdateSettings { settings: RoomSettings { format: Format::Bo3, public: true, ..Default::default() } });
        assert_eq!(a.last_room().unwrap().settings.format, Format::Bo3);
        h.handle(b.id, ClientMsg::ToggleSpectator);
        assert_eq!(b.last_room().unwrap().you, Role::Player(1));
        // public listing
        let mut c = connect(&mut h, "C");
        h.handle(c.id, ClientMsg::ListRooms);
        assert!(c.drain().iter().any(|m| matches!(m, ServerMsg::Rooms { rooms } if rooms.len() == 1 && rooms[0].code == code)));
        // full room -> spectator; disabled spectators -> error
        h.handle(a.id, ClientMsg::UpdateSettings { settings: RoomSettings { allow_spectators: false, ..Default::default() } });
        h.handle(c.id, ClientMsg::JoinRoom { code: code.clone(), spectate: false });
        assert!(c.drain().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("full"))));
        h.handle(c.id, ClientMsg::JoinRoom { code: "ZZZZZZ".into(), spectate: false });
        assert!(c.drain().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("not found"))));
        // host leaves -> host transfers
        h.handle(a.id, ClientMsg::LeaveRoom);
        let r = b.last_room().unwrap();
        assert_eq!(r.host, b.id);
    }

    #[test]
    fn matchmaking_pairs_same_mode() {
        let db = db();
        let mut h = Hub::new(db.clone());
        let mut a = connect(&mut h, "A");
        let mut b = connect(&mut h, "B");
        let mut c = connect(&mut h, "C");
        let bo1 = QueueMode { format: Format::Bo1, rules: DeckRules::Standard };
        let bo3 = QueueMode { format: Format::Bo3, rules: DeckRules::Standard };
        let deck = starter_deck(&db, "SFD-181").unwrap();
        // illegal deck rejected
        h.handle(a.id, ClientMsg::Queue { mode: bo1.clone(), deck: Deck::default(), sealed: None });
        assert!(a.drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
        h.handle(a.id, ClientMsg::Queue { mode: bo1.clone(), deck: deck.clone(), sealed: None });
        assert!(a.drain().iter().any(|m| matches!(m, ServerMsg::Queue { status } if status.searching)));
        h.handle(c.id, ClientMsg::Queue { mode: bo3, deck: deck.clone(), sealed: None });
        assert_eq!(h.queue_len(), 2);
        h.handle(b.id, ClientMsg::Queue { mode: bo1, deck: deck.clone(), sealed: None });
        assert_eq!(h.queue_len(), 1);
        let am = a.drain();
        assert!(am.iter().any(|m| matches!(m, ServerMsg::MatchFound { .. })));
        assert!(am.iter().any(|m| matches!(m, ServerMsg::Match { .. })));
        assert!(b.drain().iter().any(|m| matches!(m, ServerMsg::Match { .. })));
        h.handle(c.id, ClientMsg::CancelQueue);
        assert_eq!(h.queue_len(), 0);
        let _ = c.drain();
        // Sealed queue refused
        h.handle(c.id, ClientMsg::Queue { mode: QueueMode { format: Format::Bo1, rules: DeckRules::Sealed }, deck, sealed: None });
        assert!(c.drain().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
    }

    fn guest(h: &mut Hub, token: Option<&str>) -> C {
        let (tx, rx) = unbounded_channel();
        let (id, conn) = h.hello(None, token, tx);
        C { id, conn, rx }
    }

    #[test]
    fn guests_can_only_watch() {
        let db = db();
        let mut h = Hub::new(db.clone());
        let mut a = connect(&mut h, "Alice");
        let tok = token_of(&mut a);
        h.handle(a.id, ClientMsg::CreateRoom { settings: RoomSettings { public: true, ..Default::default() } });
        let code = a.last_room().unwrap().code;
        let mut g = guest(&mut h, None);
        let w = g.drain();
        assert!(w.iter().any(|m| matches!(m, ServerMsg::Welcome { signed_in: false, .. })));
        for m in [
            ClientMsg::CreateRoom { settings: RoomSettings::default() },
            ClientMsg::JoinRoom { code: code.clone(), spectate: false },
            ClientMsg::Queue { mode: QueueMode { format: Format::Bo1, rules: DeckRules::Standard }, deck: starter_deck(&db, "SFD-181").unwrap(), sealed: None },
            ClientMsg::Chat { text: "hi".into() },
        ] {
            h.handle(g.id, m);
            assert!(g.drain().iter().any(|m| matches!(m, ServerMsg::SignInRequired { .. })));
        }
        assert_eq!(h.queue_len(), 0);
        h.handle(g.id, ClientMsg::JoinRoom { code: code.clone(), spectate: true });
        let r = g.last_room().unwrap();
        assert_eq!(r.you, Role::Spectator);
        h.handle(g.id, ClientMsg::ToggleSpectator);
        assert!(g.drain().iter().any(|m| matches!(m, ServerMsg::SignInRequired { .. })));
        // A guest token cannot be used to take over a signed-in client.
        let mut g2 = guest(&mut h, Some(&tok));
        assert_ne!(g2.id, a.id);
        assert!(g2.drain().iter().any(|m| matches!(m, ServerMsg::Welcome { signed_in: false, .. })));
    }

    #[test]
    fn same_account_new_tab_replaces_old_socket() {
        let db = db();
        let mut h = Hub::new(db);
        let mut a = connect(&mut h, "Alice");
        let mut b = connect(&mut h, "Bob");
        assert_ne!(a.id, b.id);
        let a2 = connect(&mut h, "Alice");
        assert_eq!(a2.id, a.id);
        assert!(a.drain().iter().any(|m| matches!(m, ServerMsg::Replaced)));
        assert!(!b.drain().iter().any(|m| matches!(m, ServerMsg::Replaced)));
    }

    #[test]
    fn rename_updates_room() {
        let db = db();
        let mut h = Hub::new(db);
        let mut a = connect(&mut h, "Alice");
        h.handle(a.id, ClientMsg::CreateRoom { settings: RoomSettings::default() });
        let code = a.last_room().unwrap().code;
        let mut b = connect(&mut h, "Bob");
        h.handle(b.id, ClientMsg::JoinRoom { code, spectate: false });
        b.drain();
        let uid = rift_shared::rng::seed_from_str("Alice") as i64 & 0xffff_ffff;
        h.rename_user(uid, "Alicia");
        let r = b.last_room().unwrap();
        assert_eq!(r.players[0].name, "Alicia");
        assert_eq!(r.players[1].name, "Bob");
    }

    #[test]
    fn reconnect_with_token_keeps_seat() {
        let db = db();
        let mut h = Hub::new(db);
        let mut a = connect(&mut h, "A");
        h.handle(a.id, ClientMsg::CreateRoom { settings: RoomSettings::default() });
        let code = a.last_room().unwrap().code;
        h.disconnect(a.id, a.conn);
        let (tx, rx) = unbounded_channel();
        let uid = rift_shared::rng::seed_from_str("A") as i64 & 0xffff_ffff;
        let (id2, _) = h.hello(Some(Identity { user_id: uid, name: "A".into() }), None, tx);
        assert_eq!(id2, a.id);
        let mut a2 = C { id: id2, conn: 0, rx };
        let r = a2.last_room().unwrap();
        assert_eq!(r.code, code);
        assert!(r.players[0].connected);
        // an old socket closing after reconnect doesn't disconnect the new one
        h.disconnect(a.id, a.conn);
        h.handle(a.id, ClientMsg::Ping);
        assert!(a2.drain().iter().any(|m| matches!(m, ServerMsg::Pong)));
    }

    #[test]
    fn sealed_rooms_check_pool() {
        let db = db();
        let mut h = Hub::new(db.clone());
        let mut a = connect(&mut h, "A");
        h.handle(a.id, ClientMsg::CreateRoom { settings: RoomSettings { rules: DeckRules::Sealed, ..Default::default() } });
        let sets = sealed::default_sets("OGN");
        let pool = sealed::open_pool(&db, &sets, "s1");
        let mut deck = sealed::auto_build(&db, &pool).unwrap();
        // pad to the sealed minimum with pool cards regardless of domain for the test
        let have = pool.cards();
        for (c, n) in have {
            let d = db.get(&c).unwrap();
            if deck.main_count() >= 25 {
                break;
            }
            if d.is_main_deck_card() && !deck.main.iter().any(|e| e.0 == c) {
                deck.main.push((c, n));
            }
        }
        h.handle(a.id, ClientMsg::SetDeck { deck: deck.clone(), sealed: None });
        let r = a.last_room().unwrap();
        assert!(r.players[0].deck_errors.iter().any(|e| e.contains("sealed pool")));
        h.handle(a.id, ClientMsg::SetDeck { deck: deck.clone(), sealed: Some(SealedRef { sets: sets.clone(), seed: "other-seed".into() }) });
        let r = a.last_room().unwrap();
        assert!(r.players[0].deck_errors.iter().any(|e| e.contains("not in this sealed pool")));
        h.handle(a.id, ClientMsg::SetDeck { deck, sealed: Some(SealedRef { sets, seed: "s1".into() }) });
        let r = a.last_room().unwrap();
        assert!(!r.players[0].deck_errors.iter().any(|e| e.contains("sealed pool")), "{:?}", r.players[0].deck_errors);
    }
}
