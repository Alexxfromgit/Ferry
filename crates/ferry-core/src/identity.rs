use std::path::Path;

use anyhow::Context;
use iroh::SecretKey;

/// Load the node's secret key, creating it on first run.
///
/// The key must be stable: the node id derived from it is part of every share link, so a new
/// key would break all links handed out before.
pub fn load_or_create_secret(path: &Path) -> anyhow::Result<SecretKey> {
    if path.exists() {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let bytes = hex::decode(text.trim()).context("secret key file is not valid hex")?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret key file must contain 32 bytes"))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::util::write_atomic_private(path, hex::encode(key.to_bytes()).as_bytes())?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        let a = load_or_create_secret(&path).unwrap();
        let b = load_or_create_secret(&path).unwrap();
        assert_eq!(a.public(), b.public());
    }
}
