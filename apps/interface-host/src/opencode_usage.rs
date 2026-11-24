use async_trait::async_trait;
use engine_acp::opencode_usage::OpenCodeGoUsageReader;
use interface_contracts::engine::{EngineId, EngineUsage};

use crate::session_management::SupplementalUsageReader;

/// Supplies the Rust-owned, credential-free Go allowance reader as the
/// `acp:opencode` supplemental usage row.
#[async_trait]
impl SupplementalUsageReader for OpenCodeGoUsageReader {
    fn engine(&self) -> EngineId {
        EngineId::new("acp:opencode").expect("built-in OpenCode engine id")
    }

    async fn read_usage(&self) -> EngineUsage {
        OpenCodeGoUsageReader::read_usage(self).await
    }
}
