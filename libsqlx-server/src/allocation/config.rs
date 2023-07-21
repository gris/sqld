use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::linc::NodeId;

/// Structural supertype of AllocConfig, used for checking the meta version. Subsequent version of
/// AllocConfig need to conform to this prototype.
#[derive(Debug, Serialize, Deserialize)]
struct ConfigVersion {
    config_version: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AllocConfig {
    pub max_conccurent_connection: u32,
    pub db_name: String,
    pub db_config: DbConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum DbConfig {
    Primary {
        /// maximum size the replication log is allowed to grow, before it is compacted.
        max_log_size: usize,
        /// Interval at which to force compaction
        replication_log_compact_interval: Option<Duration>,
    },
    Replica {
        primary_node_id: NodeId,
        proxy_request_timeout_duration: Duration,
    },
}
