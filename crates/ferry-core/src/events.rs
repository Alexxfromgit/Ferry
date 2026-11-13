use serde::{Deserialize, Serialize};

use crate::registry::{DownloadInfo, ShareInfo};

/// How bytes travel between the two peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionKind {
    /// Peer-to-peer (LAN, or a hole-punched path through NAT).
    Direct,
    /// Through an encrypted relay server; works everywhere but is slower.
    Relay,
}

/// Everything the UI needs to stay up to date. Serialized as `{ "type": "...", ... }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    /// Hashing progress while a new share is being created.
    ImportProgress {
        op_id: u64,
        done_bytes: u64,
        total_bytes: u64,
        current_file: String,
    },
    ShareCreated {
        share: ShareInfo,
    },
    ShareRemoved {
        share_id: String,
    },
    /// Upload activity of a share (throttled snapshots).
    ShareActivity {
        share_id: String,
        active_peers: usize,
        uploaded_bytes: u64,
    },
    /// Any change of a download, including progress (throttled snapshots).
    DownloadUpdated {
        download: DownloadInfo,
    },
    DownloadRemoved {
        download_id: String,
    },
}
