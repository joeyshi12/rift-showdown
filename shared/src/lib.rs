//! Shared logic for Rift Online: card database, deck codes, decklists,
//! the tabletop game engine, sealed pack generation and the network protocol.
//! Compiled both natively (server, tests) and to wasm32 (client).

pub mod cards;
pub mod deckcode;
pub mod decklist;
pub mod game;
pub mod precons;
pub mod protocol;
pub mod rng;
pub mod sealed;
pub mod session;

pub use cards::{CardDb, CardDef};
pub use decklist::{Deck, DeckRules};
pub use game::{Action, GameState, Zone};
