# protocol-changelog

## v1.0 → v1.1

- **type: schema** — Added `SessionCurrentState` enum (`Idle`, `Working`, `WaitingInput`) and `SessionState` struct (`current_state`, `last_event_kind`, `last_event_at_ms`) for per-session current-state projection. Story 1.6, FR25.
