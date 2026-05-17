use crate::{error::Result, event::EventEnvelope};

pub trait SourceAdapter {
    fn meta(&self) -> AdapterMeta;
    fn normalize(&self, hook_kind: &str, raw: &[u8]) -> Result<NormalizeResult>;
}

pub struct AdapterMeta {
    pub source: &'static str,
}

pub struct NormalizeResult {
    pub envelope: EventEnvelope,
}
