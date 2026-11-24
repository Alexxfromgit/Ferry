//! Binary `.ferry` files: `MAGIC`, a version byte, a kind byte, then the link payload, which
//! names only the sender's device and the share's token. A file for a password-locked share
//! is encrypted with that password (age, scrypt), so it reveals nothing without it; the sender
//! checks the password again when the recipient connects.

use std::{fs::File, io::Read, path::Path};

use age::secrecy::ExposeSecret;
pub use age::secrecy::SecretString as TicketPassword;
use anyhow::{Context, bail, ensure};

use crate::{ShareLink, link::PAYLOAD_MAX, write_atomic};

const MAGIC: &[u8] = b"\x89FERRY\r\n";
const VERSION: u8 = 2;
const PLAIN: u8 = 0;
const PROTECTED: u8 = 1;
/// A protected file is the payload plus age's header and tag; anything this large is not one.
const MAX_FILE: usize = 4 * 1024;
// age uses scrypt r=8, p=1. N=2^17 uses 128 MiB on both sender and recipient.
const SCRYPT_WORK_FACTOR: u8 = 17;

pub enum TicketFile {
    Ready(ShareLink),
    Protected(ProtectedTicket),
}

pub struct ProtectedTicket(Vec<u8>);

/// Save a `.ferry` file for `link` atomically, encrypted with `password` if one is given.
pub fn save_ticket_file(
    path: &Path,
    link: &ShareLink,
    password: Option<TicketPassword>,
) -> anyhow::Result<()> {
    write_atomic(path, &encode(link, password)?)
}

fn encode(link: &ShareLink, password: Option<TicketPassword>) -> anyhow::Result<Vec<u8>> {
    let payload = link
        .to_bytes()
        .context("a ticket file needs the share's full link")?;
    let mut bytes = MAGIC.to_vec();
    bytes.push(VERSION);
    match password {
        Some(password) => {
            ensure!(
                !password.expose_secret().trim().is_empty(),
                "Enter a password"
            );
            let mut recipient = age::scrypt::Recipient::new(password);
            recipient.set_work_factor(SCRYPT_WORK_FACTOR);
            bytes.push(PROTECTED);
            bytes.extend(age::encrypt(&recipient, &payload)?);
        }
        None => {
            bytes.push(PLAIN);
            bytes.extend(payload);
        }
    }
    Ok(bytes)
}

impl TicketFile {
    /// Open a ticket file, with a bounded read even if the file grows.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        ensure!(path.metadata()?.is_file(), "Choose a Ferry ticket file");
        let file = File::open(path)?;
        ensure!(file.metadata()?.is_file(), "Choose a Ferry ticket file");
        Self::read_from(file)
    }

    /// Read a ticket from an open stream, such as one picked through a mobile document picker
    /// that hands out content URIs rather than paths. The read is bounded.
    pub fn read_from(reader: impl Read) -> anyhow::Result<Self> {
        let mut bytes = Vec::new();
        reader.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
        Self::decode(bytes)
    }

    fn decode(mut bytes: Vec<u8>) -> anyhow::Result<Self> {
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
        match *kind {
            PLAIN => Ok(Self::Ready(decode_payload(payload)?)),
            PROTECTED => {
                let header = age::Decryptor::new(payload).context("Invalid protected ticket")?;
                ensure!(header.is_scrypt(), "Invalid password-protected ticket");
                Ok(Self::Protected(ProtectedTicket(
                    bytes.split_off(MAGIC.len() + 2),
                )))
            }
            _ => bail!("Unknown Ferry ticket format"),
        }
    }
}

impl ProtectedTicket {
    pub fn unlock(&self, password: TicketPassword) -> anyhow::Result<ShareLink> {
        let mut identity = age::scrypt::Identity::new(password);
        identity.set_max_work_factor(SCRYPT_WORK_FACTOR);
        let decryptor = age::Decryptor::new(self.0.as_slice())?;
        let mut reader = match decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity)) {
            Err(age::DecryptError::DecryptionFailed | age::DecryptError::NoMatchingKeys) => {
                bail!("Incorrect password or damaged ticket. Check the password and try again.")
            }
            Err(age::DecryptError::ExcessiveWork { .. }) => {
                bail!("This protected ticket uses an unsupported password work factor")
            }
            result => result.context("Couldn't unlock the protected ticket")?,
        };
        let mut payload = Vec::new();
        reader
            .by_ref()
            .take(PAYLOAD_MAX as u64 + 1)
            .read_to_end(&mut payload)
            .context("Damaged protected ticket")?;
        decode_payload(&payload)
    }
}

fn decode_payload(payload: &[u8]) -> anyhow::Result<ShareLink> {
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
    fn plain_files_are_binary_and_hold_only_the_link() {
        let link = sample();
        let bytes = encode(&link, None).unwrap();
        assert_eq!(
            bytes.len(),
            MAGIC.len() + 2 + link.to_bytes().unwrap().len()
        );
        let TicketFile::Ready(read) = TicketFile::decode(bytes.clone()).unwrap() else {
            panic!("ordinary tickets open without a password");
        };
        assert_eq!(read, link);

        let mut other_version = bytes.clone();
        other_version[MAGIC.len()] = 1;
        assert!(TicketFile::decode(other_version).is_err());
        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(TicketFile::decode(truncated).is_err());
        assert!(TicketFile::decode(b"{\"ticket\": \"x\"}".to_vec()).is_err());
        assert!(encode(&ShareLink::Code("K7X2QP9A4MZT".into()), None).is_err());
    }

    #[test]
    fn protected_ticket_requires_password_and_detects_tampering() {
        let link = sample();
        let password = "  海 — winter crossing  ";
        let password = || TicketPassword::from(password.to_owned());
        let bytes = encode(&link, Some(password())).unwrap();
        let payload = link.to_bytes().unwrap();
        assert!(
            !bytes.windows(payload.len()).any(|w| w == payload),
            "the link must not appear in the clear"
        );
        let TicketFile::Protected(ticket) = TicketFile::decode(bytes).unwrap() else {
            panic!("protected tickets require a password");
        };
        let wrong_password = ticket
            .unlock(TicketPassword::from("wrong".to_owned()))
            .unwrap_err();
        assert!(wrong_password.to_string().contains("Incorrect password"));
        assert_eq!(ticket.unlock(password()).unwrap(), link);

        let mut damaged = ticket;
        *damaged.0.last_mut().unwrap() ^= 1;
        assert!(damaged.unlock(password()).is_err());
    }
}
