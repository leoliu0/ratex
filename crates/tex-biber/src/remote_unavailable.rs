//! Builds without the `remote` feature (WebAssembly has no sockets): names
//! Biber would download are recognized as remote, and fetching one fails as
//! an unreachable host does with the transport.
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default)]
pub(crate) struct Cache;

impl Cache {
    pub(crate) fn fetch(&mut self, source: &str, _options: &BTreeMap<String, String>) -> Result<Option<Arc<[u8]>>, String> {
        if !is_remote(source) { return Ok(None); }
        Err(format!("Could not fetch '{source}' (remote datasources are unavailable in this build)"))
    }
}

pub(crate) fn is_remote(source: &str) -> bool {
    ["http://", "https://", "ftp://", "ftps://"].iter().any(|prefix| source.starts_with(prefix))
}
