//! Versioned binary `.ferry` files. Version 1 stores a gzip-compressed JSON payload,
//! optionally wrapped in passphrase-based age encryption.

use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

use age::secrecy::ExposeSecret;
pub use age::secrecy::SecretString as TicketPassword;
use anyhow::{Context, bail, ensure};
use flate2::{Compression, bufread::GzDecoder, write::GzEncoder};

use crate::{ShareFile, ShareLink, write_atomic};

const MAGIC: &[u8] = b"\x89FERRY\r\n";
const VERSION: u8 = 1;
const PLAIN: u8 = 0;
const PROTECTED: u8 = 1;
// A capped sample of 1,000 long paths can be several MiB before compression.
const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
const MAX_FILE: usize = MAX_PAYLOAD + 64 * 1024; // Allow for compression and encryption overhead.
// age uses scrypt r=8, p=1. N=2^17 uses 128 MiB on both sender and recipient.
const SCRYPT_WORK_FACTOR: u8 = 17;

pub enum TicketFile {
    Ready(ShareLink),
    Protected(ProtectedTicket),
}

pub struct ProtectedTicket(Vec<u8>);

impl ShareFile {
    /// Save the ticket atomically, optionally protecting the entire payload with a password.
    pub fn save(&self, path: &Path, password: Option<TicketPassword>) -> anyhow::Result<()> {
        write_atomic(path, &self.encode(password)?)
    }

    fn encode(&self, password: Option<TicketPassword>) -> anyhow::Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        ensure!(json.len() <= MAX_PAYLOAD, "Ferry ticket is too large");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&json)?;
        let compressed = encoder.finish()?;

        let mut bytes = MAGIC.to_vec();
        bytes.push(VERSION);
        if let Some(password) = password {
            ensure!(
                !password.expose_secret().trim().is_empty(),
                "Enter a password"
            );
            let mut recipient = age::scrypt::Recipient::new(password);
            recipient.set_work_factor(SCRYPT_WORK_FACTOR);
            bytes.push(PROTECTED);
            bytes.extend(age::encrypt(&recipient, &compressed)?);
        } else {
            bytes.push(PLAIN);
            bytes.extend(compressed);
        }
        ensure!(bytes.len() <= MAX_FILE, "Ferry ticket is too large");
        Ok(bytes)
    }
}

impl TicketFile {
    /// Open a binary or legacy JSON ticket file, with a bounded read even if the file grows.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        ensure!(path.metadata()?.is_file(), "Choose a Ferry ticket file");
        let file = File::open(path)?;
        ensure!(file.metadata()?.is_file(), "Choose a Ferry ticket file");
        let mut bytes = Vec::new();
        file.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
        Self::decode(bytes)
    }

    fn decode(mut bytes: Vec<u8>) -> anyhow::Result<Self> {
        ensure!(bytes.len() <= MAX_FILE, "Ferry ticket is too large");
        let Some(binary) = bytes.strip_prefix(MAGIC) else {
            let text = std::str::from_utf8(&bytes).context("invalid Ferry ticket file")?;
            return Ok(Self::Ready(ShareLink::parse(text)?));
        };
        let [version, kind, payload @ ..] = binary else {
            bail!("Incomplete Ferry ticket header");
        };
        ensure!(
            *version == VERSION,
            "Unsupported Ferry ticket version. Update Ferry to open this file."
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
            .read_to_end(&mut payload)
            .context("Damaged protected ticket")?;
        decode_payload(&payload)
    }
}

fn decode_payload(payload: &[u8]) -> anyhow::Result<ShareLink> {
    let mut decoder = GzDecoder::new(payload);
    let mut json = String::new();
    (&mut decoder)
        .take(MAX_PAYLOAD as u64 + 1)
        .read_to_string(&mut json)
        .context("Damaged Ferry ticket")?;
    ensure!(
        json.len() <= MAX_PAYLOAD,
        "Ferry ticket payload is too large"
    );
    ensure!(
        decoder.get_ref().is_empty(),
        "Unexpected data after the Ferry ticket"
    );
    ensure!(json.starts_with('{'), "Invalid Ferry ticket payload");
    ShareLink::parse(&json)
}

#[cfg(test)]
mod tests {
    use iroh::{EndpointAddr, SecretKey};
    use iroh_blobs::{BlobFormat, Hash, ticket::BlobTicket};

    use super::*;

    fn sample() -> ShareFile {
        let addr = EndpointAddr::new(SecretKey::generate().public());
        let ticket = BlobTicket::new(addr, Hash::new(b"hello"), BlobFormat::HashSeq);
        ShareFile::new(&ticket, "Private photos".into(), 5, &[])
    }

    #[test]
    fn reads_binary_and_legacy_tickets() {
        let file = sample();
        let binary = file.encode(None).unwrap();
        assert!(std::str::from_utf8(&binary).is_err());
        for bytes in [binary.clone(), file.to_json().into_bytes()] {
            let TicketFile::Ready(link) = TicketFile::decode(bytes).unwrap() else {
                panic!("ordinary tickets open without a password");
            };
            assert_eq!(link.ticket.to_string(), file.ticket);
        }

        let mut future = binary.clone();
        future[MAGIC.len()] += 1;
        assert!(TicketFile::decode(future).is_err());
        let mut truncated = binary.clone();
        truncated.pop();
        assert!(TicketFile::decode(truncated).is_err());
        let mut damaged = binary;
        *damaged.last_mut().unwrap() ^= 1;
        assert!(TicketFile::decode(damaged).is_err());
    }

    #[test]
    fn protected_ticket_requires_password_and_detects_tampering() {
        let file = sample();
        let password = "  海 — winter crossing  ";
        let password = || TicketPassword::from(password.to_owned());
        let bytes = file.encode(Some(password())).unwrap();
        let TicketFile::Protected(ticket) = TicketFile::decode(bytes).unwrap() else {
            panic!("protected tickets require a password");
        };
        let wrong_password = ticket
            .unlock(TicketPassword::from("wrong".to_owned()))
            .unwrap_err();
        assert!(wrong_password.to_string().contains("Incorrect password"));
        assert_eq!(
            ticket.unlock(password()).unwrap().ticket.to_string(),
            file.ticket
        );

        let mut damaged = ticket;
        *damaged.0.last_mut().unwrap() ^= 1;
        assert!(damaged.unlock(password()).is_err());
    }
}
