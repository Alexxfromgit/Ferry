//! Share passwords: stored only as Argon2id hashes, checked by the sender, with a limit on how
//! fast anyone can guess.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::Context;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};

/// The shortest password a share accepts.
pub const MIN_PASSWORD_CHARS: usize = 4;

/// Hash a new share password (PHC string).
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        password.chars().count() >= MIN_PASSWORD_CHARS,
        "Use a password of at least {MIN_PASSWORD_CHARS} characters"
    );
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt)
        .map_err(|e| anyhow::anyhow!("no randomness for the password salt: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("could not protect the password: {e}"))?
        .to_string())
}

/// Whether `password` matches a hash made by [`hash_password`]. Takes a few tens of
/// milliseconds on purpose; run it off the async threads.
pub fn verify_password(hash: &str, password: &str) -> anyhow::Result<bool> {
    let hash = PasswordHash::new(hash)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("the share's password record is damaged")?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok())
}

/// Guesses allowed in a burst, per share.
const BURST: f64 = 5.0;
/// One more guess becomes available this often, per share.
const REFILL: Duration = Duration::from_secs(10);

/// Limits password guesses per share (not per device: device ids cost nothing to make), so a
/// leaked link can't be used to try passwords quickly. About six guesses a minute.
#[derive(Default)]
pub struct GuessLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl GuessLimiter {
    /// Take one guess for `share`: `Ok` if allowed, or how long to wait.
    pub fn take(&self, share: &str) -> Result<(), Duration> {
        self.take_at(share, Instant::now())
    }

    fn take_at(&self, share: &str, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(share.to_string()).or_insert(Bucket {
            tokens: BURST,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated);
        bucket.tokens = (bucket.tokens + elapsed.as_secs_f64() / REFILL.as_secs_f64()).min(BURST);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(REFILL.mul_f64(1.0 - bucket.tokens))
        }
    }

    /// Forget a share's guesses (when it is removed or gets a new password).
    pub fn reset(&self, share: &str) {
        self.buckets.lock().unwrap().remove(share);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_hash_and_verify() {
        let hash = hash_password("winter crossing").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(!hash.contains("winter"));
        assert!(verify_password(&hash, "winter crossing").unwrap());
        assert!(!verify_password(&hash, "winter crossinG").unwrap());
        assert!(hash_password("abc").is_err());
        assert!(verify_password("not a hash", "x").is_err());
    }

    #[test]
    fn guesses_are_limited_per_share() {
        let limiter = GuessLimiter::default();
        let start = Instant::now();
        for _ in 0..5 {
            assert!(limiter.take_at("a", start).is_ok());
        }
        let wait = limiter.take_at("a", start).unwrap_err();
        assert!(wait <= REFILL && wait > Duration::ZERO, "{wait:?}");
        // Other shares are not affected.
        assert!(limiter.take_at("b", start).is_ok());
        // One more guess after the refill time.
        assert!(limiter.take_at("a", start + REFILL).is_ok());
        assert!(limiter.take_at("a", start + REFILL).is_err());
        limiter.reset("a");
        assert!(limiter.take_at("a", start + REFILL).is_ok());
    }
}
