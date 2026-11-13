use std::{path::PathBuf, str::FromStr};

use anyhow::Context;
use iroh::{RelayMode, RelayUrl};
use serde::{Deserialize, Serialize};

/// Which relay servers the node uses when a direct connection is not possible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "url", rename_all = "camelCase")]
pub enum RelayChoice {
    /// The free public relays operated by n0 (no SLA, rate limited).
    #[default]
    Default,
    /// A single self-hosted `iroh-relay`, e.g. `https://relay.example.com`.
    Custom(String),
    /// No relays: only direct connections (LAN, or hosts reachable from the internet).
    Disabled,
}

impl RelayChoice {
    /// Check that a custom relay address is usable, so a typo can't stop the node from starting.
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Self::Custom(url) = self {
            validate_relay_url(url)?;
        }
        Ok(())
    }

    pub(crate) fn to_relay_mode(&self) -> anyhow::Result<RelayMode> {
        Ok(match self {
            Self::Default => RelayMode::Default,
            Self::Disabled => RelayMode::Disabled,
            Self::Custom(url) => RelayMode::Custom(validate_relay_url(url)?.into()),
        })
    }
}

/// Parse a relay address: `http(s)://host[:port][/]`.
pub fn validate_relay_url(url: &str) -> anyhow::Result<RelayUrl> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    anyhow::ensure!(
        lower.starts_with("https://") || lower.starts_with("http://"),
        "relay address must start with https:// (or http:// for testing)"
    );
    let parsed = RelayUrl::from_str(url).context("relay address is not a valid URL")?;
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', ':']).next().unwrap_or(""))
        .unwrap_or("");
    anyhow::ensure!(!host.is_empty(), "relay address has no host name");
    Ok(parsed)
}

/// Node configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where the identity key, blob store and registry live.
    pub data_dir: PathBuf,
    /// Relay servers to use. An invalid custom relay falls back to the default relays.
    pub relay: RelayChoice,
    /// Publish and resolve node addresses via n0's DNS/pkarr service, so links keep working
    /// after the sender's IP address changes. Disable only for offline tests.
    pub address_lookup: bool,
    /// Resume downloads that were running when the node last stopped.
    pub resume_downloads: bool,
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            relay: RelayChoice::Default,
            address_lookup: true,
            resume_downloads: true,
        }
    }

    /// Fully offline configuration used by tests: no relays and no address lookup.
    pub fn local_only(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            relay: RelayChoice::Disabled,
            address_lookup: false,
            resume_downloads: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_urls_are_validated() {
        assert!(validate_relay_url("https://relay.example.com").is_ok());
        assert!(validate_relay_url("https://relay.example.com:8443/").is_ok());
        assert!(validate_relay_url("http://10.0.0.5:3340").is_ok());
        for bad in [
            "https://",
            "https://relay example.com",
            "relay.example.com",
            "ftp://relay.example.com",
            "",
        ] {
            assert!(
                validate_relay_url(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }
}
