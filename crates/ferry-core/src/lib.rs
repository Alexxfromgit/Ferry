//! Ferry engine: share files and folders peer-to-peer.
//!
//! Built on [iroh](https://iroh.computer) (connections dialed by public key, with hole punching
//! and relay fallback) and `iroh-blobs` (BLAKE3-verified, resumable transfers).
//!
//! The crate is UI-agnostic: the CLI and the Tauri desktop app are thin layers over [`Node`].

mod config;
mod events;
mod identity;
mod link;
mod node;
mod provider;
mod registry;
mod ticket_file;
mod util;

pub use config::{Config, RelayChoice, validate_relay_url};
pub use events::{ConnectionKind, Event};
pub use link::{
    FileEntry, LINK_SCHEME, MAX_FILES_IN_SHARE_FILE, SHARE_FILE_EXTENSION, ShareFile, ShareLink,
};
pub use node::{Node, NodeStatus, PREVIEW_FILES, Preview};
pub use registry::{DownloadInfo, DownloadStatus, ShareInfo, SkipReason, SkippedItem};
pub use ticket_file::{ProtectedTicket, TicketFile, TicketPassword};
pub use util::write_atomic;
