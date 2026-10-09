//! Provider-agnostic core: the item model, the `Provider` trait, fuzzy
//! matching, frecency ranking and the `Engine` that ties them together.

pub mod engine;
pub mod frecency;
pub mod item;
pub mod matcher;
pub mod provider;
pub mod secrets;

pub use engine::Engine;
pub use engine::Outcome;
pub use item::{Action, ActionKind, Hit, Icon, Item, Prompt};
pub use matcher::Matcher;
pub use provider::{Provider, Query};
