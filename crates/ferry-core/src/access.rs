//! Share passwords: stored only as Argon2id hashes, checked by the sender, with a limit on how
//! fast anyone can guess.

use std::{
    collections::HashMap,
    hash::Hash,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::Context;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use iroh::EndpointId;

/// The shortest password a share accepts, not counting spaces at either end.
pub const MIN_PASSWORD_CHARS: usize = 4;

/// Whether `password` is acceptable for a share, with a message saying why not.
pub fn check_password(password: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        password.trim().chars().count() >= MIN_PASSWORD_CHARS,
        "Use a password of at least {MIN_PASSWORD_CHARS} characters"
    );
    Ok(())
}

/// Hash a new share password (PHC string).
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    check_password(password)?;
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

/// A token bucket's size and refill rate.
#[derive(Clone, Copy)]
struct Rate {
    burst: f64,
    refill: Duration,
}

/// Each device: a few guesses, then one every ten seconds.
const PER_DEVICE: Rate = Rate {
    burst: 5.0,
    refill: Duration::from_secs(10),
};
/// Each share, from all devices together: device ids cost nothing to make, so this caps how
/// fast a leaked link can be used to guess, at about 30 a minute.
const PER_SHARE: Rate = Rate {
    burst: 30.0,
    refill: Duration::from_secs(2),
};
/// Buckets kept before full ones are forgotten (a full bucket is the same as none).
const MAX_BUCKETS: usize = 4096;

struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl Bucket {
    fn refill(&mut self, rate: Rate, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated);
        self.tokens =
            (self.tokens + elapsed.as_secs_f64() / rate.refill.as_secs_f64()).min(rate.burst);
        self.updated = now;
    }

    /// How long until one guess is available.
    fn wait(&self, rate: Rate) -> Duration {
        rate.refill.mul_f64((1.0 - self.tokens).max(0.0))
    }
}

struct Buckets<K> {
    rate: Rate,
    map: HashMap<K, Bucket>,
}

impl<K: Eq + Hash + Clone> Buckets<K> {
    fn new(rate: Rate) -> Self {
        Self {
            rate,
            map: HashMap::new(),
        }
    }

    fn get(&mut self, key: &K, now: Instant) -> &mut Bucket {
        if self.map.len() >= MAX_BUCKETS && !self.map.contains_key(key) {
            let rate = self.rate;
            self.map.retain(|_, b| {
                b.refill(rate, now);
                b.tokens < rate.burst
            });
        }
        let rate = self.rate;
        let bucket = self.map.entry(key.clone()).or_insert(Bucket {
            tokens: rate.burst,
            updated: now,
        });
        bucket.refill(rate, now);
        bucket
    }
}

/// Limits password guesses per device and per share. A guess is taken before the password is
/// checked (so parallel guesses can't slip past the limit) and given back if it was right, so
/// only wrong passwords count.
pub struct GuessLimiter {
    inner: Mutex<Limits>,
}

/// Buckets per (share, device) and per share.
struct Limits {
    devices: Buckets<(String, EndpointId)>,
    shares: Buckets<String>,
}

impl Default for GuessLimiter {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Limits {
                devices: Buckets::new(PER_DEVICE),
                shares: Buckets::new(PER_SHARE),
            }),
        }
    }
}

impl GuessLimiter {
    /// Take one guess at `share` from `device`: `Ok` if allowed, or how long to wait.
    pub fn take(&self, share: &str, device: EndpointId) -> Result<(), Duration> {
        self.take_at(share, device, Instant::now())
    }

    fn take_at(&self, share: &str, device: EndpointId, now: Instant) -> Result<(), Duration> {
        let mut guard = self.inner.lock().unwrap();
        let Limits { devices, shares } = &mut *guard;
        let (device_rate, share_rate) = (devices.rate, shares.rate);
        let device_wait = {
            let bucket = devices.get(&(share.to_string(), device), now);
            (bucket.tokens < 1.0).then(|| bucket.wait(device_rate))
        };
        let share_wait = {
            let bucket = shares.get(&share.to_string(), now);
            (bucket.tokens < 1.0).then(|| bucket.wait(share_rate))
        };
        if let Some(wait) = device_wait.max(share_wait) {
            return Err(wait);
        }
        devices.get(&(share.to_string(), device), now).tokens -= 1.0;
        shares.get(&share.to_string(), now).tokens -= 1.0;
        Ok(())
    }

    /// Give back a guess that turned out to be right.
    pub fn give_back(&self, share: &str, device: EndpointId) {
        let mut guard = self.inner.lock().unwrap();
        let Limits { devices, shares } = &mut *guard;
        let now = Instant::now();
        let (device_rate, share_rate) = (devices.rate, shares.rate);
        let bucket = devices.get(&(share.to_string(), device), now);
        bucket.tokens = (bucket.tokens + 1.0).min(device_rate.burst);
        let bucket = shares.get(&share.to_string(), now);
        bucket.tokens = (bucket.tokens + 1.0).min(share_rate.burst);
    }

    /// Forget a share's guesses (when it is removed or gets a new password).
    pub fn reset(&self, share: &str) {
        let mut guard = self.inner.lock().unwrap();
        let Limits { devices, shares } = &mut *guard;
        devices.map.retain(|(s, _), _| s != share);
        shares.map.remove(share);
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    #[test]
    fn passwords_hash_and_verify() {
        let hash = hash_password("winter crossing").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(!hash.contains("winter"));
        assert!(verify_password(&hash, "winter crossing").unwrap());
        assert!(!verify_password(&hash, "winter crossinG").unwrap());
        assert!(hash_password("abc").is_err());
        assert!(
            hash_password("      ").is_err(),
            "spaces alone are not a password"
        );
        assert!(hash_password("  ab  ").is_err());
        assert!(verify_password("not a hash", "x").is_err());
    }

    #[test]
    fn each_device_gets_a_few_guesses() {
        let limiter = GuessLimiter::default();
        let (alice, mallory) = (
            SecretKey::generate().public(),
            SecretKey::generate().public(),
        );
        let start = Instant::now();
        for _ in 0..5 {
            assert!(limiter.take_at("a", mallory, start).is_ok());
        }
        let wait = limiter.take_at("a", mallory, start).unwrap_err();
        assert!(
            wait <= PER_DEVICE.refill && wait > Duration::ZERO,
            "{wait:?}"
        );
        // Someone else guessing doesn't lock out the real recipient.
        assert!(limiter.take_at("a", alice, start).is_ok());
        // Another share is not affected either.
        assert!(limiter.take_at("b", mallory, start).is_ok());
        // One more guess after the refill time.
        assert!(
            limiter
                .take_at("a", mallory, start + PER_DEVICE.refill)
                .is_ok()
        );
        assert!(
            limiter
                .take_at("a", mallory, start + PER_DEVICE.refill)
                .is_err()
        );
        limiter.reset("a");
        assert!(
            limiter
                .take_at("a", mallory, start + PER_DEVICE.refill)
                .is_ok()
        );
    }

    #[test]
    fn many_devices_together_are_capped_per_share() {
        let limiter = GuessLimiter::default();
        let start = Instant::now();
        let mut allowed = 0;
        for _ in 0..100 {
            if limiter
                .take_at("a", SecretKey::generate().public(), start)
                .is_ok()
            {
                allowed += 1;
            }
        }
        assert_eq!(allowed, PER_SHARE.burst as usize);
    }

    #[test]
    fn right_passwords_cost_nothing() {
        let limiter = GuessLimiter::default();
        let alice = SecretKey::generate().public();
        let start = Instant::now();
        for _ in 0..20 {
            limiter.take_at("a", alice, start).unwrap();
            limiter.give_back("a", alice);
        }
    }
}
