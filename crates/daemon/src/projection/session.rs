use std::time::{SystemTime, UNIX_EPOCH};

use protocol::{EventEnvelope, EventId, Reaction};
use rusqlite::params;
use serde_json::Value;

use crate::db::queries::{INSERT_EVENT, UPSERT_PROJECTION};
use crate::db::DbPools;
use crate::error::{Error, Result};

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn kind_string(envelope: &EventEnvelope) -> Result<String> {
    serde_json::to_value(&envelope.kind)
        .ok()
        .and_then(|v: Value| v.as_str().map(|s| s.to_owned()))
        .ok_or_else(|| Error::Config("EventKind serialization failed".into()))
}

fn reaction_string(reaction: &Option<Reaction>) -> Option<String> {
    reaction
        .as_ref()
        .and_then(|r| serde_json::to_value(r).ok())
        .and_then(|v: Value| v.as_str().map(|s| s.to_owned()))
}

fn placeholder_projection_state(envelope: &EventEnvelope, ts: i64) -> String {
    let kind = kind_string(envelope).unwrap_or_else(|_| "Unknown".to_owned());
    format!("{{\"last_kind\":\"{kind}\",\"updated_at\":{ts}}}")
}

#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]
pub async fn write(pools: &DbPools, envelope: EventEnvelope) -> Result<EventId> {
    let conn = pools.writer.get().await?;
    let event_id: i64 = conn
        .interact(move |c| -> Result<i64> {
            let ts = unix_ms();
            let state = placeholder_projection_state(&envelope, ts);
            let kind_str = kind_string(&envelope)?;
            let reaction_str = reaction_string(&envelope.reaction);

            let tx = c.transaction()?;
            tx.execute(
                UPSERT_PROJECTION,
                params![envelope.source, envelope.session_id, state, ts],
            )?;
            let event_id: i64 = tx.query_row(
                INSERT_EVENT,
                params![
                    envelope.source,
                    envelope.session_id,
                    kind_str,
                    reaction_str,
                    envelope.payload,
                    ts,
                ],
                |row| row.get(0),
            )?;
            tx.commit()?;
            Ok(event_id)
        })
        .await??;
    Ok(EventId(event_id))
}
