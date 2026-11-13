//! Share links and `.ferry` share files: the three forms a share can travel in.

use std::str::FromStr;

use anyhow::Context;
use iroh_blobs::{BlobFormat, ticket::BlobTicket};
use serde::{Deserialize, Serialize};

/// URL scheme registered by the desktop app (`ferry://get?t=<ticket>`).
pub const LINK_SCHEME: &str = "ferry";
/// Extension of share files (the ".torrent file" of Ferry).
pub const SHARE_FILE_EXTENSION: &str = "ferry";

const SHARE_FILE_VERSION: u32 = 1;

/// A parsed share: what a recipient needs to download.
#[derive(Debug, Clone)]
pub struct ShareLink {
    pub ticket: BlobTicket,
}

impl ShareLink {
    pub fn new(ticket: BlobTicket) -> Self {
        Self { ticket }
    }

    /// `ferry://get?t=<ticket>`; opens the app when clicked.
    pub fn to_url(&self) -> String {
        format!("{LINK_SCHEME}://get?t={}", self.ticket)
    }

    /// Accepts any of the forms a user may paste or open:
    /// a `ferry://` link, a raw ticket, or the JSON contents of a `.ferry` file.
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        let input = input.trim();
        if input.starts_with('{') {
            let file: ShareFile = serde_json::from_str(input).context("invalid .ferry file")?;
            anyhow::ensure!(
                file.ferry <= SHARE_FILE_VERSION,
                "this ticket file was made by a newer version of Ferry; please update Ferry"
            );
            return Self::parse_text(&file.ticket);
        }
        Self::parse_text(input)
    }

    /// A `ferry://` link or a raw ticket (never JSON).
    fn parse_text(input: &str) -> anyhow::Result<Self> {
        let input = input.trim();
        let ticket_text = if let Some(rest) = strip_scheme(input) {
            let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("t="))
                .context("link has no ticket (expected ferry://get?t=...)")?
        } else {
            input
        };
        let ticket = BlobTicket::from_str(ticket_text.trim()).context("invalid share ticket")?;
        anyhow::ensure!(
            ticket.format() == BlobFormat::HashSeq,
            "this ticket does not point to a Ferry share"
        );
        Ok(Self { ticket })
    }
}

fn strip_scheme(input: &str) -> Option<&str> {
    let (scheme, rest) = input.split_once("://")?;
    scheme.eq_ignore_ascii_case(LINK_SCHEME).then_some(rest)
}

/// One file inside a share, as shown in previews and share files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// `/`-separated relative path, e.g. `Holiday/IMG_001.jpg`.
    pub name: String,
    pub size: u64,
}

/// Payload of a `.ferry` ticket file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFile {
    /// Format version of this file.
    #[serde(default = "default_version")]
    pub ferry: u32,
    pub ticket: String,
    /// Display name of the share (top-level file or folder name).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub total_size: u64,
    /// Number of files in the share.
    #[serde(default)]
    pub file_count: usize,
    /// The first [`MAX_FILES_IN_SHARE_FILE`] files, for people curious what's inside. The
    /// recipient's app fetches the real list from the sender.
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Keeps `.ferry` files small enough to email even for shares with very many files.
pub const MAX_FILES_IN_SHARE_FILE: usize = 1000;

fn default_version() -> u32 {
    SHARE_FILE_VERSION
}

impl ShareFile {
    pub fn new(ticket: &BlobTicket, name: String, total_size: u64, files: &[FileEntry]) -> Self {
        Self {
            ferry: SHARE_FILE_VERSION,
            ticket: ticket.to_string(),
            name,
            total_size,
            file_count: files.len(),
            files: files
                .iter()
                .take(MAX_FILES_IN_SHARE_FILE)
                .cloned()
                .collect(),
            note: None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("ShareFile serializes")
    }
}

#[cfg(test)]
mod tests {
    use iroh::{EndpointAddr, SecretKey};
    use iroh_blobs::Hash;

    use super::*;

    fn ticket() -> BlobTicket {
        let addr = EndpointAddr::new(SecretKey::generate().public());
        BlobTicket::new(addr, Hash::new(b"hello"), BlobFormat::HashSeq)
    }

    #[test]
    fn parses_all_forms() {
        let t = ticket();
        let link = ShareLink::new(t.clone());
        let file = ShareFile::new(&t, "x".into(), 0, &[]);
        for input in [
            t.to_string(),
            link.to_url(),
            format!("  FERRY://get?t={t}\n"),
            file.to_json(),
        ] {
            let parsed = ShareLink::parse(&input).unwrap();
            assert_eq!(parsed.ticket.hash(), t.hash());
        }
    }

    #[test]
    fn share_files_minimal_future_and_nested() {
        let t = ticket();
        let minimal = format!("{{\"ticket\": \"{t}\"}}");
        assert!(ShareLink::parse(&minimal).is_ok());
        let future = format!("{{\"ferry\": 2, \"ticket\": \"{t}\"}}");
        assert!(ShareLink::parse(&future).is_err());
        let nested = r#"{"ticket": "{\"ticket\": \"x\"}"}"#;
        assert!(ShareLink::parse(nested).is_err());
    }

    #[test]
    fn rejects_garbage_and_raw_blobs() {
        assert!(ShareLink::parse("hello").is_err());
        assert!(ShareLink::parse("ferry://get").is_err());
        let addr = EndpointAddr::new(SecretKey::generate().public());
        let raw = BlobTicket::new(addr, Hash::new(b"x"), BlobFormat::Raw);
        assert!(ShareLink::parse(&raw.to_string()).is_err());
    }
}
