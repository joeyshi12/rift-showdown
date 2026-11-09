//! WebSocket protocol between client and server (JSON, serde externally tagged).

use crate::decklist::{Deck, DeckRules};
use crate::game::Mode;
use crate::session::{Format, MatchAction, MatchView};
use serde::{Deserialize, Serialize};

pub const MAX_CHAT_LEN: usize = 300;
pub const MAX_NAME_LEN: usize = 24;
pub const MAX_MSG_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RoomSettings {
    pub format: Format,
    pub rules: DeckRules,
    /// Listed in the public room browser.
    pub public: bool,
    pub allow_spectators: bool,
    pub spectators_see_hands: bool,
    pub auto_steps: bool,
    pub win_points: i32,
    /// 1v1, 2v2, or three- or four-player free-for-all.
    #[serde(default)]
    pub mode: Mode,
}

impl Default for RoomSettings {
    fn default() -> Self {
        RoomSettings { format: Format::Bo1, rules: DeckRules::Standard, public: false, allow_spectators: true, spectators_see_hands: false, auto_steps: true, win_points: crate::game::DEFAULT_WIN_POINTS, mode: Mode::Duel }
    }
}

/// Sealed pool reference so the server can verify a sealed deck.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SealedRef {
    pub sets: Vec<String>,
    pub seed: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct QueueMode {
    pub format: Format,
    pub rules: DeckRules,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientMsg {
    /// First message on every socket. `token` resumes an earlier connection (so a
    /// dropped socket keeps its seat); `name` is the player's chosen display name.
    Hello { token: Option<String>, name: String },
    /// Change the display name. Rooms show it at once; a match in progress keeps
    /// the names it started with.
    SetName { name: String },
    CreateRoom { settings: RoomSettings },
    JoinRoom { code: String, spectate: bool },
    LeaveRoom,
    UpdateSettings { settings: RoomSettings },
    SetDeck { deck: Deck, sealed: Option<SealedRef> },
    SetReady { ready: bool },
    /// Move between a player seat and the spectator list.
    ToggleSpectator,
    /// 2v2 lobby: join team 0 (A) or 1 (B).
    SetTeam { team: u8 },
    Act { action: MatchAction },
    ReturnToLobby,
    Chat { text: String },
    ListRooms,
    Queue { mode: QueueMode, deck: Deck, sealed: Option<SealedRef> },
    CancelQueue,
    Ping,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MemberView {
    pub id: u64,
    pub name: String,
    pub connected: bool,
    pub ready: bool,
    pub deck_name: Option<String>,
    pub legend: Option<String>,
    pub deck_ok: bool,
    pub deck_errors: Vec<String>,
    /// 2v2 team (0 = A, 1 = B); 0 in other modes.
    #[serde(default)]
    pub team: u8,
    /// Left during a 2v2 / free-for-all match; the seat stays until the match ends.
    #[serde(default)]
    pub left: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Role {
    Player(u8),
    Spectator,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RoomView {
    pub code: String,
    pub settings: RoomSettings,
    pub host: u64,
    pub players: Vec<MemberView>,
    pub spectators: Vec<MemberView>,
    pub in_game: bool,
    pub from_queue: bool,
    pub you: Role,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RoomSummary {
    pub code: String,
    pub host_name: String,
    pub players: usize,
    pub spectators: usize,
    pub in_game: bool,
    pub settings: RoomSettings,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChatLine {
    pub from: String,
    pub text: String,
    pub system: bool,
    pub ts: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct QueueStatus {
    pub searching: bool,
    pub mode: Option<QueueMode>,
    pub waited_secs: u64,
    pub in_queue: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ServerMsg {
    Welcome { id: u64, token: String, name: String },
    /// The same session reconnected on another socket; this one is retired.
    Replaced,
    Error { message: String },
    Info { message: String },
    Room { room: RoomView },
    LeftRoom,
    Match { view: Box<MatchView> },
    Chat { line: ChatLine },
    ChatHistory { lines: Vec<ChatLine> },
    Rooms { rooms: Vec<RoomSummary> },
    Queue { status: QueueStatus },
    MatchFound { code: String },
    Pong,
}

/// Trim and limit a display name; empty becomes "Player".
pub fn clean_name(s: &str) -> String {
    let n: String = s.chars().filter(|c| !c.is_control()).take(MAX_NAME_LEN).collect();
    let n = n.trim().to_string();
    if n.is_empty() {
        "Player".into()
    } else {
        n
    }
}

pub fn clean_chat(s: &str) -> Option<String> {
    let t: String = s.chars().filter(|c| !c.is_control() || *c == ' ').take(MAX_CHAT_LEN).collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Action, Zone};

    #[test]
    fn json_roundtrip() {
        let msgs = vec![
            ClientMsg::Hello { token: None, name: "Ana".into() },
            ClientMsg::SetName { name: "Bo".into() },
            ClientMsg::Act { action: MatchAction::Game(Action::Move { card: 3, to: Zone::Bf(1, 0), pos: Default::default() }) },
            ClientMsg::Queue { mode: QueueMode { format: Format::Bo3, rules: DeckRules::Standard }, deck: Deck::default(), sealed: None },
        ];
        for m in msgs {
            let s = serde_json::to_string(&m).unwrap();
            let back: ClientMsg = serde_json::from_str(&s).unwrap();
            assert_eq!(back, m);
        }
        let mv: ClientMsg = serde_json::from_str(r#"{"Act":{"action":{"Game":{"Move":{"card":1,"to":{"Hand":0}}}}}}"#).unwrap();
        assert!(matches!(mv, ClientMsg::Act { .. }));
    }

    #[test]
    fn cleaning() {
        assert_eq!(clean_name("   "), "Player");
        assert_eq!(clean_name(&"a".repeat(100)).len(), MAX_NAME_LEN);
        assert_eq!(clean_chat("  hi\u{7}  "), Some("hi".into()));
        assert_eq!(clean_chat("   "), None);
    }
}
