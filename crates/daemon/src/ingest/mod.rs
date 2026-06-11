pub mod handler;
pub mod listener;
pub mod writer;

pub use listener::run as listener_run;
pub use writer::run as writer_run;

use protocol::EventEnvelope;

/// Where an ingested envelope came from. Story 5.11 / ADR 0009 §7: live shim
/// hooks run the PID-supersession follow-up; `/replay` does NOT (the synthetic
/// `SessionEnded` rows are already in the log being replayed — re-running
/// supersession would double-generate them and could end the current live PID
/// holder on replay arrival order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOrigin {
    /// A real-time hook event from the shim ingest socket.
    Live,
    /// A reconstructed event pushed by `POST /replay`.
    Replay,
}

/// One item on the ingest channel: the envelope plus where it originated.
/// Both the live shim listener and the `/replay` endpoint push these onto the
/// single channel drained by [`writer::run`], which dispatches on `origin`.
#[derive(Debug, Clone)]
pub struct IngestItem {
    pub envelope: EventEnvelope,
    pub origin: IngestOrigin,
}
