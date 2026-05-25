# Cookbook

Recipes for common presenter problems. Each entry pairs with one of the [reference examples](../../examples/) under `examples/` — the example is the canonical implementation, the cookbook entry is the explanation. A CI test (`tests/cli_docs_drift.rs::cookbook_include_directives_match_example_anchors`) asserts the code block in each entry is byte-identical to the anchored region in its paired example, so the docs cannot rot independently of the source.

| Cookbook entry | Paired example | The problem |
|----------------|---------------|-------------|
| [state-session-fanout.md](state-session-fanout.md) | [`multi-session-router`](../../examples/multi-session-router/) | I need to track every session as it appears and route state to a per-session model. |
| [rest-cursor-pagination.md](rest-cursor-pagination.md) | [`event-log-viewer`](../../examples/event-log-viewer/) | I need to fetch a session's history via REST and handle event-log truncation gracefully. |
| [dropped-frame-recovery.md](dropped-frame-recovery.md) | [`reconnect-recovery`](../../examples/reconnect-recovery/) | My WebSocket dropped or the daemon restarted; how do I catch up without losing events? |

More recipes will follow as patterns emerge. Open an issue if you have a use case the existing three don't cover.
