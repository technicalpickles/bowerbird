# Deferred Work

## Deferred from: code review of 1-1-workspace-and-protocol-crate-foundation (2026-05-17)

- **SourceAdapter `&'static str` rigidity** — `AdapterMeta.source` is `&'static str`, preventing runtime-configured or dynamically loaded adapters. Consider a newtype or owned `String` when adapter discovery is implemented. [crates/protocol/src/adapter.rs]
- **Payload String no schema enforcement** — `EventEnvelope.payload` and `Event.payload` are opaque `String` with no validation that content is valid JSON or matches `kind`. Consider per-kind typed payload enums or validation in the adapter layer. [crates/protocol/src/event.rs]
- **Reaction::Vendor error message ambiguity** — `Vendor(65536)` (u16 overflow) and `Vendor(abc)` (non-numeric) produce the same error string. Add distinct error messages distinguishing parse failure vs range overflow. [crates/protocol/src/reaction.rs]
- **SyncFrame ordering not validated** — No guard that `oldest_available_event_id <= latest_event_id`. Add validation in daemon when SyncFrame is constructed. [crates/protocol/src/ws.rs]
- **DroppedFrame invariants not validated** — `count`, `first_dropped_event_id`, `last_dropped_event_id` have no relational guards. Add validation in daemon when DroppedFrame is constructed. [crates/protocol/src/ws.rs]
- **ClientMessage empty topic accepted** — `Subscribe { topic: String }` accepts empty strings. Add non-empty validation in daemon topic routing. [crates/protocol/src/ws.rs]
- **CI matrix omits Windows** — `keyring` has a Windows backend; add a Windows runner when Windows is a supported target. [.github/workflows/ci.yml]
