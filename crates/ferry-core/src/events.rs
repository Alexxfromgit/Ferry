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

/// How far one receiver of a share has got, as seen by the sender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverProgress {
    /// Unique among all transfers since Ferry started; stays the same while this one runs.
    pub id: u64,
    /// Bytes sent in this transfer.
    pub done_bytes: u64,
    /// Bytes this transfer asked for. A resumed download only asks for what it is missing.
    pub total_bytes: u64,
    pub bytes_per_sec: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
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
    /// A share changed, for example it was turned off or on.
    ShareUpdated {
        share: ShareInfo,
    },
    /// Upload activity of a share (throttled snapshots, sent while anything changes).
    ShareActivity {
        share_id: String,
        active_peers: usize,
        uploaded_bytes: u64,
        /// Combined upload speed of all receivers.
        bytes_per_sec: u64,
        /// One entry per running download of this share's files.
        receivers: Vec<ReceiverProgress>,
    },
    /// Any change of a download, including progress (throttled snapshots).
    DownloadUpdated {
        download: DownloadInfo,
    },
    DownloadRemoved {
        download_id: String,
    },
}
