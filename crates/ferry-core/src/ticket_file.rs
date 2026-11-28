//! Binary `.ferry` files: `MAGIC`, a version byte, a kind byte, then the link payload, which
//! names only the sender's device and the share's token (and, without relays, its addresses).
//!
//! A locked share's file is the same: its token gets nobody in without the password, which the
//! sender checks. Encrypting the file with that password would only let whoever holds the file
//! guess the password offline, without the sender's limit on guesses.

use std::{fs::File, io::Read, path::Path};

use anyhow::{Context, bail, ensure};

use crate::{ShareLink, link::PAYLOAD_MAX, write_atomic};

const MAGIC: &[u8] = b"\x89FERRY\r\n";
const VERSION: u8 = 2;
/// The only kind so far; the byte leaves room for others.
const PLAIN: u8 = 0;
const MAX_FILE: usize = MAGIC.len() + 2 + PAYLOAD_MAX;

/// Save a `.ferry` file for `link` atomically.
pub fn save_ticket_file(path: &Path, link: &ShareLink) -> anyhow::Result<()> {
    write_atomic(path, &encode(link)?)
}

fn encode(link: &ShareLink) -> anyhow::Result<Vec<u8>> {
    let payload = link
        .to_bytes()
        .context("a ticket file needs the share's full link")?;
    let mut bytes = MAGIC.to_vec();
    bytes.push(VERSION);
    bytes.push(PLAIN);
    bytes.extend(payload);
    Ok(bytes)
}

/// Open a ticket file, with a bounded read even if the file grows.
pub fn read_ticket_file(path: &Path) -> anyhow::Result<ShareLink> {
    ensure!(path.metadata()?.is_file(), "Choose a Ferry ticket file");
    let file = File::open(path)?;
    ensure!(file.metadata()?.is_file(), "Choose a Ferry ticket file");
    read_ticket_from(file)
}

/// Read a ticket from an open stream, such as one picked through a mobile document picker
/// that hands out content URIs rather than paths. The read is bounded.
pub fn read_ticket_from(reader: impl Read) -> anyhow::Result<ShareLink> {
    let mut bytes = Vec::new();
    reader.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> anyhow::Result<ShareLink> {
    ensure!(bytes.len() <= MAX_FILE, "This is not a Ferry ticket file");
    let binary = bytes
        .strip_prefix(MAGIC)
        .context("This is not a Ferry ticket file")?;
    let [version, kind, payload @ ..] = binary else {
        bail!("Incomplete Ferry ticket header");
    };
    ensure!(
        *version == VERSION,
        "This ticket file is from another version of Ferry. Ask the sender for a new one."
    );
    ensure!(
        *kind == PLAIN,
        "This ticket file is from a newer version of Ferry. Update Ferry to open it."
    );
    ShareLink::from_bytes(payload).context("Damaged Ferry ticket")
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;
    use crate::link::ShareToken;

    fn sample() -> ShareLink {
        ShareLink::direct(SecretKey::generate().public(), ShareToken::generate())
    }

    #[test]
    fn files_are_binary_and_hold_only_the_link() {
        let link = sample();
        let bytes = encode(&link).unwrap();
        assert_eq!(
            bytes.len(),
            MAGIC.len() + 2 + link.to_bytes().unwrap().len()
        );
        assert!(std::str::from_utf8(&bytes).is_err(), "not readable as text");
        assert_eq!(decode(&bytes).unwrap(), link);

        let mut other_version = bytes.clone();
        other_version[MAGIC.len()] = 1;
        assert!(decode(&other_version).is_err());
        let mut other_kind = bytes.clone();
        other_kind[MAGIC.len() + 1] = 1;
        assert!(decode(&other_kind).is_err());
        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(decode(&truncated).is_err());
        assert!(decode(b"{\"ticket\": \"x\"}").is_err());
        assert!(encode(&ShareLink::Code("K7X2QP9A4MZT".into())).is_err());
    }

    #[test]
    fn reads_are_bounded() {
        let mut bytes = encode(&sample()).unwrap();
        bytes.extend(std::iter::repeat_n(0, MAX_FILE));
        assert!(read_ticket_from(bytes.as_slice()).is_err());
    }
}
