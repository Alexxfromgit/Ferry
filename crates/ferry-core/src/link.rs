//! Share links: the forms a share travels in.
//!
//! A link names the sender's device and a random per-share token, never the content hash.
//! It carries network addresses only when the sender uses no relays, since nothing else could
//! find the device then:
//!
//! - `https://ferry.xenvaultinteractive.com/s#<payload>`: self-contained. The payload sits in
//!   the fragment, which browsers never send to the server.
//! - `https://ferry.xenvaultinteractive.com/s/<CODE>`: a short code that the link service
//!   resolves to the same device and token (see `docs/LINKS.md`).
//! - `ferry://s#<payload>` and `ferry://s/<CODE>`: the same links for an installed app.
//!
//! The recipient connects to the device and presents the token; the sender decides whether to
//! serve it (and may ask for a password first).

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use anyhow::{Context, bail, ensure};
use data_encoding::BASE64URL_NOPAD;
use iroh::EndpointId;
use serde::{Deserialize, Serialize};

/// URL scheme registered by the apps (`ferry://s/<CODE>`).
pub const LINK_SCHEME: &str = "ferry";
/// Host of the web link. Phones open these in the app through verified App Links; anywhere
/// else the page offers to open or install Ferry.
pub const WEB_LINK_HOST: &str = "ferry.xenvaultinteractive.com";
const LINK_PATH: &str = "s";
/// Extension of share files.
pub const SHARE_FILE_EXTENSION: &str = "ferry";

/// Version byte at the start of a link payload.
const PAYLOAD_VERSION: u8 = 1;
const TOKEN_LEN: usize = 16;
/// Version, device and token; addresses may follow.
const PAYLOAD_MIN: usize = 1 + 32 + TOKEN_LEN;
/// Addresses a link carries at most.
const MAX_ADDRS: usize = 8;
/// The longest payload: every address an IPv6 one (tag, address, port).
pub(crate) const PAYLOAD_MAX: usize = PAYLOAD_MIN + MAX_ADDRS * (1 + 16 + 2);
const ADDR_V4: u8 = 4;
const ADDR_V6: u8 = 6;

/// Short codes: 12 characters of Crockford base32 (60 bits).
pub const CODE_LEN: usize = 12;
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Domain separation for code derivation; shared with the link service.
const CODE_CONTEXT: &[u8] = b"ferry-link-v1";

/// A random secret that names one share. Whoever has it can ask the sender for the share.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShareToken(#[serde(with = "hex_token")] pub [u8; TOKEN_LEN]);

impl ShareToken {
    pub fn generate() -> Self {
        let mut bytes = [0u8; TOKEN_LEN];
        getrandom::fill(&mut bytes).expect("the OS random number generator works");
        Self(bytes)
    }
}

impl fmt::Debug for ShareToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Tokens grant access; keep them out of logs.
        f.write_str("ShareToken(..)")
    }
}

mod hex_token {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8; 16], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 16], D::Error> {
        let text = String::deserialize(d)?;
        let bytes = hex::decode(text).map_err(D::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| D::Error::custom("a share token has 16 bytes"))
    }
}

/// A parsed share link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareLink {
    /// Names the device and the share itself.
    Direct {
        endpoint: EndpointId,
        token: ShareToken,
        /// Where to reach the device, only when it uses no relays (empty otherwise).
        addrs: Vec<SocketAddr>,
    },
    /// A short code the link service resolves into a [`ShareLink::Direct`].
    Code(String),
}

impl ShareLink {
    pub fn direct(endpoint: EndpointId, token: ShareToken) -> Self {
        Self::Direct {
            endpoint,
            token,
            addrs: Vec::new(),
        }
    }

    /// A direct link that also carries `addrs` (at most eight, IPv4 first).
    pub fn with_addrs(endpoint: EndpointId, token: ShareToken, addrs: &[SocketAddr]) -> Self {
        let mut addrs = addrs.to_vec();
        // Sorted fully, so `dedup` sees every duplicate next to its twin.
        addrs.sort_by_key(|a| (a.is_ipv6(), a.ip().is_loopback(), *a));
        addrs.dedup();
        addrs.truncate(MAX_ADDRS);
        Self::Direct {
            endpoint,
            token,
            addrs,
        }
    }

    /// The `https://` link Ferry shows, copies and puts in QR codes, because it works from any
    /// camera, chat app or browser.
    pub fn to_url(&self) -> String {
        match self {
            Self::Direct { .. } => {
                format!("https://{WEB_LINK_HOST}/{LINK_PATH}#{}", self.payload())
            }
            Self::Code(code) => format!("https://{WEB_LINK_HOST}/{LINK_PATH}/{code}"),
        }
    }

    /// The `ferry://` link that opens an installed app directly.
    pub fn to_app_url(&self) -> String {
        match self {
            Self::Direct { .. } => format!("{LINK_SCHEME}://{LINK_PATH}#{}", self.payload()),
            Self::Code(code) => format!("{LINK_SCHEME}://{LINK_PATH}/{code}"),
        }
    }

    /// The short code of this link: given for [`ShareLink::Code`], derived for a direct link.
    pub fn code(&self) -> String {
        match self {
            Self::Direct {
                endpoint, token, ..
            } => share_code(endpoint, token),
            Self::Code(code) => code.clone(),
        }
    }

    /// The binary form used inside link fragments and `.ferry` files. Short codes have none.
    pub(crate) fn to_bytes(&self) -> Option<Vec<u8>> {
        let Self::Direct {
            endpoint,
            token,
            addrs,
        } = self
        else {
            return None;
        };
        let mut bytes = Vec::with_capacity(PAYLOAD_MIN + addrs.len() * 19);
        bytes.push(PAYLOAD_VERSION);
        bytes.extend_from_slice(endpoint.as_bytes());
        bytes.extend_from_slice(&token.0);
        for addr in addrs.iter().take(MAX_ADDRS) {
            match addr.ip() {
                IpAddr::V4(ip) => {
                    bytes.push(ADDR_V4);
                    bytes.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    bytes.push(ADDR_V6);
                    bytes.extend_from_slice(&ip.octets());
                }
            }
            bytes.extend_from_slice(&addr.port().to_be_bytes());
        }
        Some(bytes)
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let Some((&version, _)) = bytes.split_first() else {
            bail!("this link is incomplete");
        };
        ensure!(
            version == PAYLOAD_VERSION,
            "this link was made by a newer version of Ferry; please update Ferry"
        );
        ensure!(
            (PAYLOAD_MIN..=PAYLOAD_MAX).contains(&bytes.len()),
            "this link is damaged"
        );
        let endpoint_bytes: [u8; 32] = bytes[1..33].try_into().expect("length checked");
        let endpoint = EndpointId::from_bytes(&endpoint_bytes).context("this link is damaged")?;
        let token = ShareToken(bytes[33..PAYLOAD_MIN].try_into().expect("length checked"));
        let mut rest = &bytes[PAYLOAD_MIN..];
        let mut addrs = Vec::new();
        while let Some((&tag, tail)) = rest.split_first() {
            let len = match tag {
                ADDR_V4 => 4,
                ADDR_V6 => 16,
                _ => bail!("this link is damaged"),
            };
            ensure!(tail.len() >= len + 2, "this link is damaged");
            let ip = if len == 4 {
                IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&tail[..4]).expect("4")))
            } else {
                IpAddr::V6(Ipv6Addr::from(
                    <[u8; 16]>::try_from(&tail[..16]).expect("16"),
                ))
            };
            let port = u16::from_be_bytes([tail[len], tail[len + 1]]);
            addrs.push(SocketAddr::new(ip, port));
            rest = &tail[len + 2..];
        }
        Ok(Self::Direct {
            endpoint,
            token,
            addrs,
        })
    }

    fn payload(&self) -> String {
        BASE64URL_NOPAD.encode(&self.to_bytes().expect("only direct links have a payload"))
    }

    /// Accepts any form a user may paste or open: a web link, a `ferry://` link, or a bare
    /// short code (with or without dashes).
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        let input = input.trim();
        let rest = if let Some(rest) = strip_scheme(input) {
            rest
        } else if let Some(rest) = strip_web_prefix(input) {
            rest
        } else if let Some(code) = normalize_code(input) {
            return Ok(Self::Code(code));
        } else {
            bail!("this is not a Ferry link");
        };
        // `rest` starts right after `s`: `/<CODE>`, `#<payload>` or `/#<payload>`.
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        if let Some(payload) = rest.strip_prefix('#') {
            let payload = payload.trim();
            ensure!(!payload.is_empty(), "this link is incomplete");
            let bytes = BASE64URL_NOPAD
                .decode(payload.as_bytes())
                .ok()
                .context("this link is damaged")?;
            return Self::from_bytes(&bytes);
        }
        let code = rest.split(['?', '#']).next().unwrap_or_default();
        ensure!(!code.is_empty(), "this link is incomplete");
        normalize_code(code)
            .map(Self::Code)
            .context("this link is damaged")
    }
}

/// The short code of a share: 60 bits of `BLAKE3("ferry-link-v1" ‖ endpoint ‖ token)` in
/// Crockford base32, most significant bits first. The link service stores codes made this way
/// and the recipient recomputes it, so the service can't point a code at another device.
pub fn share_code(endpoint: &EndpointId, token: &ShareToken) -> String {
    derive_code(endpoint.as_bytes(), &token.0)
}

fn derive_code(endpoint: &[u8; 32], token: &[u8; TOKEN_LEN]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CODE_CONTEXT);
    hasher.update(endpoint);
    hasher.update(token);
    let hash = hasher.finalize();
    let bits = u64::from_be_bytes(hash.as_bytes()[..8].try_into().expect("8 bytes"));
    (0..CODE_LEN)
        .map(|i| CROCKFORD[((bits >> (64 - 5 * (i + 1))) & 31) as usize] as char)
        .collect()
}

/// A code grouped for reading aloud or typing: `K7X2-QP9A-4MZT`.
pub fn format_code(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

/// A short code in canonical form (upper case, no separators), if `input` is one. Crockford
/// base32 reads `O` as `0` and `I`/`L` as `1`, so codes survive being read aloud or retyped.
fn normalize_code(input: &str) -> Option<String> {
    let code: String = input
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (code.len() == CODE_LEN && code.bytes().all(|b| CROCKFORD.contains(&b))).then_some(code)
}

/// Whether `input` is a share link (web or `ferry://`) as opposed to a bare code or a path.
/// Cheap: it doesn't validate the link.
pub fn is_share_url(input: &str) -> bool {
    let input = input.trim();
    strip_scheme(input).is_some() || strip_web_prefix(input).is_some()
}

/// Whether `input` is a `ferry://` link, the only kind the desktop deep-link handler receives.
/// Web links passed on the command line arrive only as arguments.
pub fn is_app_url(input: &str) -> bool {
    strip_scheme(input.trim()).is_some()
}

/// The part after `ferry://s`, if `input` is an app link.
fn strip_scheme(input: &str) -> Option<&str> {
    let (scheme, rest) = input.split_once("://")?;
    if !scheme.eq_ignore_ascii_case(LINK_SCHEME) {
        return None;
    }
    strip_path(rest)
}

/// The part after `https://<host>/s`, if `input` is a web link.
fn strip_web_prefix(input: &str) -> Option<&str> {
    let (scheme, rest) = input.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")) {
        return None;
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, rest) = rest.split_at(host_end);
    let host = host.strip_suffix('.').unwrap_or(host);
    if !host.eq_ignore_ascii_case(WEB_LINK_HOST) {
        return None;
    }
    strip_path(rest.strip_prefix('/')?)
}

/// `s`, followed by the end, `/`, `#` or `?`; returns what follows `s`.
fn strip_path(rest: &str) -> Option<&str> {
    let head = rest.get(..LINK_PATH.len())?;
    if !head.eq_ignore_ascii_case(LINK_PATH) {
        return None;
    }
    let rest = &rest[LINK_PATH.len()..];
    (rest.is_empty() || rest.starts_with(['/', '#', '?'])).then_some(rest)
}

/// One file inside a share, as shown in previews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// `/`-separated relative path, e.g. `Holiday/IMG_001.jpg`.
    pub name: String,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    fn direct() -> ShareLink {
        ShareLink::direct(SecretKey::generate().public(), ShareToken::generate())
    }

    #[test]
    fn direct_links_round_trip() {
        let link = direct();
        let url = link.to_url();
        assert!(url.starts_with("https://ferry.xenvaultinteractive.com/s#"));
        assert!(url.len() < 110, "{url}");
        for input in [
            url.clone(),
            link.to_app_url(),
            format!("  FERRY://S#{}\n", link.payload()),
            format!(
                "https://ferry.xenvaultinteractive.com/s/#{}",
                link.payload()
            ),
            format!(
                "https://FERRY.xenvaultinteractive.com./s#{}",
                link.payload()
            ),
        ] {
            assert_eq!(ShareLink::parse(&input).unwrap(), link, "{input}");
        }
    }

    #[test]
    fn short_codes() {
        let link = ShareLink::Code("K7X2QP9A4MZT".into());
        assert_eq!(
            link.to_url(),
            "https://ferry.xenvaultinteractive.com/s/K7X2QP9A4MZT"
        );
        for input in [
            link.to_url(),
            link.to_app_url(),
            "https://ferry.xenvaultinteractive.com/S/k7x2-qp9a-4mzt".into(),
            "HTTPS://FERRY.XENVAULTINTERACTIVE.COM/S/K7X2QP9A4MZT".into(),
            "k7x2-qp9a-4mzt".into(),
        ] {
            assert_eq!(ShareLink::parse(&input).unwrap(), link, "{input}");
        }
        // Crockford: O reads as 0, I and L as 1.
        assert_eq!(
            ShareLink::parse("K7X2QP9AOMZT").unwrap(),
            ShareLink::Code("K7X2QP9A0MZT".into())
        );
        assert_eq!(format_code("K7X2QP9A4MZT"), "K7X2-QP9A-4MZT");
    }

    #[test]
    fn codes_are_derived_from_device_and_token() {
        // Shared test vector with the link service (docs/LINKS.md): endpoint bytes
        // 0x01..=0x20, token bytes 0xA0..=0xAF.
        let endpoint: [u8; 32] = std::array::from_fn(|i| i as u8 + 1);
        let token: [u8; 16] = std::array::from_fn(|i| 0xA0 + i as u8);
        let code = derive_code(&endpoint, &token);
        assert_eq!(code, "6N76Y6BE0EQX");
        assert!(normalize_code(&code).is_some());

        let endpoint = SecretKey::generate().public();
        let token = ShareToken::generate();
        let code = share_code(&endpoint, &token);
        assert_ne!(code, share_code(&endpoint, &ShareToken::generate()));
        assert_eq!(ShareLink::direct(endpoint, token).code(), code);
    }

    #[test]
    fn rejects_other_hosts_paths_and_garbage() {
        let payload = direct().payload();
        for other in [
            format!("https://ferry.xenvaultinteractive.com.evil.test/s#{payload}"),
            format!("https://evil.test/s#{payload}"),
            format!("https://ferry.xenvaultinteractive.com/other#{payload}"),
            format!("https://ferry.xenvaultinteractive.com/sx#{payload}"),
            format!("ftp://ferry.xenvaultinteractive.com/s#{payload}"),
            format!("ferry://get?t={payload}"),
        ] {
            assert!(!is_share_url(&other), "{other}");
            assert!(ShareLink::parse(&other).is_err(), "{other}");
        }
        for bad in [
            "hello",
            "ferry://s",
            "ferry://s#",
            "https://ferry.xenvaultinteractive.com/s",
            "https://ferry.xenvaultinteractive.com/s/TOO-SHORT",
            "https://ferry.xenvaultinteractive.com/s#AAAA",
            "K7X2QP9A4MZU", // U is not Crockford
        ] {
            assert!(ShareLink::parse(bad).is_err(), "{bad}");
        }
        assert!(is_share_url("ferry://s/K7X2QP9A4MZT"));
        assert!(is_app_url(" FERRY://s#x"));
        assert!(!is_app_url("https://ferry.xenvaultinteractive.com/s#x"));
        assert!(!is_share_url(r"C:\Users\me\share.ferry"));
    }

    #[test]
    fn links_without_relays_carry_addresses() {
        let endpoint = SecretKey::generate().public();
        let token = ShareToken::generate();
        let addrs: Vec<SocketAddr> = vec![
            "[fe80::1]:4000".parse().unwrap(),
            "192.168.1.20:52000".parse().unwrap(),
            "127.0.0.1:52000".parse().unwrap(),
        ];
        let link = ShareLink::with_addrs(endpoint, token, &addrs);
        let ShareLink::Direct { addrs: kept, .. } = &link else {
            unreachable!()
        };
        assert_eq!(kept[0], addrs[1], "IPv4 comes first");
        assert_eq!(ShareLink::parse(&link.to_url()).unwrap(), link);
        // The code names the share, not where it is.
        assert_eq!(link.code(), ShareLink::direct(endpoint, token).code());
        // Duplicates go, wherever they are in the list.
        let twice: Vec<SocketAddr> = ["10.0.0.1:1", "10.0.0.2:1", "10.0.0.1:1"]
            .map(|a| a.parse().unwrap())
            .to_vec();
        let ShareLink::Direct { addrs: kept, .. } = ShareLink::with_addrs(endpoint, token, &twice)
        else {
            unreachable!()
        };
        assert_eq!(kept, twice[..2]);
        let many: Vec<SocketAddr> = (0..20)
            .map(|i| SocketAddr::from(([10, 0, 0, i], 1)))
            .collect();
        let ShareLink::Direct { addrs, .. } = ShareLink::with_addrs(endpoint, token, &many) else {
            unreachable!()
        };
        assert_eq!(addrs.len(), MAX_ADDRS);
        let mut bytes = ShareLink::direct(endpoint, token).to_bytes().unwrap();
        bytes.extend([ADDR_V4, 1, 2]);
        assert!(ShareLink::from_bytes(&bytes).is_err());
    }

    #[test]
    fn newer_payloads_ask_for_an_update() {
        let mut bytes = direct().to_bytes().unwrap();
        bytes[0] = 2;
        let err = ShareLink::from_bytes(&bytes).unwrap_err().to_string();
        assert!(err.contains("newer version"), "{err}");
    }
}
