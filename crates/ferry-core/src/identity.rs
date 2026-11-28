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
        // Keys written by Ferry 0.3 and earlier may be readable by others on Windows. Only
        // after the key was read in full, and with the same contents, so it can't be lost.
        if let Err(e) = crate::util::make_private(path, text.as_bytes()) {
            tracing::warn!("could not make {} private: {e:#}", path.display());
        }
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
    fn keys_written_before_become_private_and_stay_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        let key = SecretKey::generate();
        // As an older Ferry wrote it: readable by others.
        crate::util::write_atomic(&path, hex::encode(key.to_bytes()).as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let loaded = load_or_create_secret(&path).unwrap();
        assert_eq!(loaded.public(), key.public(), "the same key");
        assert!(
            !crate::util::make_private(&path, &std::fs::read(&path).unwrap()).unwrap(),
            "already private after loading"
        );
        assert_eq!(load_or_create_secret(&path).unwrap().public(), key.public());
    }

    #[test]
    fn key_is_stable_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        let a = load_or_create_secret(&path).unwrap();
        let b = load_or_create_secret(&path).unwrap();
        assert_eq!(a.public(), b.public());
    }
}
