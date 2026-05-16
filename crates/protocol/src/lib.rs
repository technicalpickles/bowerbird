#![deny(unsafe_code)]

mod adapter;
mod constants;
mod error;
mod event;
mod reaction;
mod rest;
mod ws;

pub use adapter::{AdapterMeta, NormalizeResult, SourceAdapter};
pub use constants::SHIM_BINARY_NAME;
pub use error::{Error, Result};
pub use event::{Event, EventEnvelope, EventId, EventKind};
pub use reaction::Reaction;
pub use rest::{EventListResponse, SessionStats};
pub use ws::{
    ClientMessage, CloseFrame, DroppedFrame, EventFrame, HelloFrame, ServerMessage, SyncFrame,
};
