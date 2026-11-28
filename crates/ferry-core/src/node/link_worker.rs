//! Keeping the link service up to date: one worker makes every request to it, one at a time and
//! paced, so a device stays under the service's write limit whatever happens at once.
//!
//! - Every share is registered, refreshed every [`LINK_REFRESH`] on its own schedule, and tried
//!   again after a failure, sooner at first and then less often. Turned-off shares stay
//!   registered, so they keep their codes.
//! - Codes to remove (of removed shares, or all of them when short links are turned off) are
//!   saved in the registry until the service confirms, so a removal that fails, or doesn't get
//!   through before Ferry quits, is tried again, also after a restart. They go before any
//!   registration.
//! - A registration that finishes after its share was removed or short links were turned off
//!   removes its code again: the worker checks that under the registry lock, which those
//!   changes take too.
//! - A short link is shown only while its registration lasts; if refreshing keeps failing,
//!   the share falls back to its full link before the service forgets the code.

use std::{
    collections::HashMap,
    sync::{Weak, atomic::Ordering},
    time::{Duration, Instant},
};

use iroh::RelayUrl;

use super::{Inner, Node};
use crate::{config::RelayChoice, events::Event, links_service::LINK_LIFETIME, util};

/// How often a share's short link is registered again, so it never expires while the share
/// exists.
pub(in crate::node) const LINK_REFRESH: Duration = Duration::from_secs(12 * 60 * 60);
/// How soon a failed request is tried again at first; the wait doubles up to an hour.
const LINK_RETRY: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(120)
};
const LINK_RETRY_MAX: Duration = Duration::from_secs(60 * 60);
/// The pause between requests, so a device with many shares stays under the service's limit
/// of 10 writes a minute per network, with room to spare for other devices there.
pub(in crate::node) const LINK_PACE: Duration = if cfg!(test) {
    Duration::from_millis(100)
} else {
    Duration::from_secs(7)
};
/// A short link stops being shown this long before its registration runs out, so nobody is
/// handed a code the service is about to forget.
pub(in crate::node) const EXPIRY_MARGIN: Duration = Duration::from_secs(24 * 60 * 60);
/// How long shutting down waits for removals still to be sent.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// What the worker knows about the service, beyond the removals saved in the registry.
#[derive(Default)]
pub(super) struct LinkState {
    /// Shares whose code the service has: share id → when the registration runs out (unix).
    pub(in crate::node) registered: HashMap<String, u64>,
    /// Shares to register: when, and the wait after the last failure, if it failed.
    pub(in crate::node) due: HashMap<String, Due>,
    /// After a failed removal: when to try again, and how long that wait was.
    removal_retry: Option<(Instant, Duration)>,
    /// When each request was sent (tests check the pacing on these).
    #[cfg(test)]
    pub(in crate::node) sent: Vec<Instant>,
}

#[derive(Clone, Copy)]
pub(in crate::node) struct Due {
    pub(in crate::node) at: Instant,
    backoff: Option<Duration>,
}

fn next_backoff(last: Option<Duration>) -> Duration {
    last.map_or(LINK_RETRY, |last| (last * 2).min(LINK_RETRY_MAX))
}

enum Job {
    Remove(String),
    Register(String),
}

impl Node {
    /// The short-link service, unless short links are off.
    pub(super) fn link_service(&self) -> Option<&crate::links_service::LinkService> {
        self.inner
            .links
            .as_ref()
            .filter(|_| !self.inner.links_off.load(Ordering::SeqCst))
    }

    /// Whether the share's code is registered and will be for a while yet.
    pub(super) fn has_short_link(&self, id: &str) -> bool {
        let margin = EXPIRY_MARGIN.as_secs();
        self.inner
            .link_state
            .lock()
            .unwrap()
            .registered
            .get(id)
            .is_some_and(|&expires| expires > util::now_unix() + margin)
    }

    /// Have the worker register a share's code (now, unless it is registered already).
    pub(super) fn schedule_short_link(&self, id: &str) {
        if !self.short_links_on() {
            return;
        }
        {
            let mut state = self.inner.link_state.lock().unwrap();
            if state.registered.contains_key(id) {
                return;
            }
            state.due.insert(
                id.to_string(),
                Due {
                    at: Instant::now(),
                    backoff: None,
                },
            );
        }
        self.inner.links_wake.notify_one();
    }

    /// A share is gone: stop registering it and have its code removed, whether or not short
    /// links are on now. A code from an earlier run may be registered even if it isn't marked,
    /// and removing an unknown code does no harm. Called under the registry lock.
    pub(super) fn forget_short_link(
        &self,
        registry: &mut crate::registry::Registry,
        id: &str,
        code: String,
    ) {
        {
            let mut state = self.inner.link_state.lock().unwrap();
            state.registered.remove(id);
            state.due.remove(id);
        }
        if self.inner.links.is_some() && !registry.link_removals.contains(&code) {
            registry.link_removals.push(code);
        }
        self.inner.links_wake.notify_one();
    }

    /// Turn short links off or back on while running.
    ///
    /// Off: shares show their full links at once, nothing more is registered or looked up, and
    /// the codes are removed from the service (saved until it confirms, so this survives a
    /// restart). On again: the shares are registered again. A node started without a link
    /// service can't turn them on; that needs a restart.
    pub fn set_short_links(&self, on: bool) {
        if self.inner.links.is_none() || self.inner.links_off.swap(!on, Ordering::SeqCst) == !on {
            return;
        }
        let shown: Vec<String> = {
            // Under the registry lock, like the worker's check after registering: a
            // registration finishing now either sees the switch, or is undone here.
            let mut registry = self.inner.registry.lock().unwrap();
            let mut state = self.inner.link_state.lock().unwrap();
            let me = self.endpoint().id();
            let codes: Vec<(String, String)> = registry
                .access
                .iter()
                .map(|(id, access)| (id.clone(), crate::link::share_code(&me, &access.token)))
                .collect();
            if on {
                // Codes of current shares are registered again rather than removed.
                registry
                    .link_removals
                    .retain(|code| !codes.iter().any(|(_, c)| c == code));
                let now = Instant::now();
                for (id, _) in &codes {
                    state.due.insert(
                        id.clone(),
                        Due {
                            at: now,
                            backoff: None,
                        },
                    );
                }
                Vec::new()
            } else {
                state.due.clear();
                for (_, code) in codes {
                    if !registry.link_removals.contains(&code) {
                        registry.link_removals.push(code);
                    }
                }
                state.registered.drain().map(|(id, _)| id).collect()
            }
        };
        self.persist();
        for id in shown {
            if let Some(share) = self.share(&id) {
                self.emit(Event::ShareUpdated { share });
            }
        }
        self.inner.links_wake.notify_one();
    }

    /// Start the worker, if this node has a link service. At start, every share is due for
    /// registration (if short links are on) and saved removals are sent first.
    pub(super) fn start_link_worker(&self) {
        if self.inner.links.is_none() {
            return;
        }
        if self.short_links_on() {
            let ids: Vec<String> = self
                .inner
                .registry
                .lock()
                .unwrap()
                .shares
                .iter()
                .map(|s| s.id.clone())
                .collect();
            let now = Instant::now();
            let mut state = self.inner.link_state.lock().unwrap();
            for id in ids {
                state.due.insert(
                    id,
                    Due {
                        at: now,
                        backoff: None,
                    },
                );
            }
        }
        let task = tokio::spawn(run(
            std::sync::Arc::downgrade(&self.inner),
            self.inner.links_wake.clone(),
        ));
        *self.inner.link_worker.lock().unwrap() =
            Some(n0_future::task::AbortOnDropHandle::new(task));
    }

    /// Let the worker send the removals still waiting, for a few seconds at most. Whatever is
    /// left stays saved and is sent on the next start.
    pub(super) async fn stop_link_worker(&self) {
        let worker = self.inner.link_worker.lock().unwrap().take();
        let Some(mut worker) = worker else { return };
        self.inner.links_closing.store(true, Ordering::SeqCst);
        self.inner.links_wake.notify_one();
        tokio::time::timeout(SHUTDOWN_GRACE, &mut worker).await.ok();
    }

    /// Take registrations that are about to run out off the shares, and say so.
    fn expire_short_links(&self) {
        let limit = util::now_unix() + EXPIRY_MARGIN.as_secs();
        let lapsed: Vec<String> = {
            let mut state = self.inner.link_state.lock().unwrap();
            let lapsed: Vec<String> = state
                .registered
                .iter()
                .filter(|(_, expires)| **expires <= limit)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &lapsed {
                state.registered.remove(id);
            }
            lapsed
        };
        for id in lapsed {
            if let Some(share) = self.share(&id) {
                self.emit(Event::ShareUpdated { share });
            }
        }
    }

    /// The next request to make, or how long until there may be one.
    fn next_link_job(&self, closing: bool) -> Result<Job, Option<Duration>> {
        let now = Instant::now();
        let mut wait: Option<Duration> = None;
        let mut sooner = |d: Duration| wait = Some(wait.map_or(d, |w| w.min(d)));
        // The registry is locked before the link state, as everywhere else.
        let removal = self
            .inner
            .registry
            .lock()
            .unwrap()
            .link_removals
            .first()
            .cloned();
        let state = self.inner.link_state.lock().unwrap();
        if let Some(code) = removal {
            match state.removal_retry {
                Some((at, _)) if at > now => sooner(at - now),
                _ => return Ok(Job::Remove(code)),
            }
        }
        if !closing && self.short_links_on() {
            if let Some((id, due)) = state.due.iter().min_by_key(|(_, due)| due.at) {
                if due.at <= now {
                    return Ok(Job::Register(id.clone()));
                }
                sooner(due.at - now);
            }
            // Wake to fall back to the full link when a registration is about to run out.
            let margin = EXPIRY_MARGIN.as_secs();
            if let Some(expires) = state.registered.values().min() {
                let left = expires.saturating_sub(util::now_unix() + margin);
                sooner(Duration::from_secs(left.max(1)));
            }
        }
        Err(wait)
    }

    async fn send_removal(&self, code: String) {
        let Some(service) = self.inner.links.clone() else {
            return;
        };
        let secret = self.endpoint().secret_key().clone();
        let result = service.delete(&secret, &code, util::now_unix()).await;
        match result {
            Ok(()) => {
                self.inner.link_state.lock().unwrap().removal_retry = None;
                self.inner
                    .registry
                    .lock()
                    .unwrap()
                    .link_removals
                    .retain(|c| *c != code);
                self.persist();
            }
            Err(e) => {
                tracing::warn!("could not remove a short link: {e:#}");
                let mut state = self.inner.link_state.lock().unwrap();
                let wait = next_backoff(state.removal_retry.map(|(_, w)| w));
                state.removal_retry = Some((Instant::now() + wait, wait));
            }
        }
    }

    async fn send_registration(&self, id: String) {
        let Some(service) = self.link_service().cloned() else {
            return;
        };
        let token = self
            .inner
            .registry
            .lock()
            .unwrap()
            .access
            .get(&id)
            .map(|a| a.token);
        let Some(token) = token else {
            self.inner.link_state.lock().unwrap().due.remove(&id);
            return;
        };
        // The home relay helps recipients find this device sooner; wait briefly for it.
        if !matches!(self.inner.relay, RelayChoice::Disabled) {
            self.wait_online(Duration::from_secs(10)).await;
        }
        let relay: Option<RelayUrl> = self.endpoint().addr().relay_urls().next().cloned();
        let expires = util::now_unix() + LINK_LIFETIME.as_secs();
        let secret = self.endpoint().secret_key().clone();
        let result = service
            .register(&secret, &token, relay.as_ref(), expires)
            .await;
        let code = match result {
            Ok(code) => code,
            Err(e) => {
                tracing::warn!(%id, "could not register a short link: {e:#}");
                let mut state = self.inner.link_state.lock().unwrap();
                if let Some(due) = state.due.get_mut(&id) {
                    let wait = next_backoff(due.backoff);
                    *due = Due {
                        at: Instant::now() + wait,
                        backoff: Some(wait),
                    };
                }
                return;
            }
        };
        // The share may have been removed, or short links turned off, while this was under way.
        // Checked under the registry lock, which those changes take too.
        let newly = {
            let mut registry = self.inner.registry.lock().unwrap();
            let still_wanted = registry.access.get(&id).is_some_and(|a| a.token == token)
                && !self.inner.links_off.load(Ordering::SeqCst);
            let mut state = self.inner.link_state.lock().unwrap();
            if still_wanted {
                state.due.insert(
                    id.clone(),
                    Due {
                        at: Instant::now() + LINK_REFRESH,
                        backoff: None,
                    },
                );
                // Shown only once it lasts beyond the margin, as `has_short_link` checks.
                Some(state.registered.insert(id.clone(), expires).is_none())
            } else {
                state.due.remove(&id);
                if !registry.link_removals.contains(&code) {
                    registry.link_removals.push(code);
                }
                None
            }
        };
        match newly {
            Some(true) => {
                if let Some(share) = self.share(&id) {
                    self.emit(Event::ShareUpdated { share });
                }
            }
            Some(false) => {}
            None => self.persist(),
        }
    }
}

/// The worker: one request at a time, [`LINK_PACE`] apart, until the node is gone or, once it
/// shuts down, no removal is left. It holds the node only while it works, never while it
/// waits.
async fn run(node: Weak<Inner>, wake: std::sync::Arc<tokio::sync::Notify>) {
    let mut last_request: Option<Instant> = None;
    loop {
        let Some(inner) = node.upgrade() else { return };
        // Wake-ups use `notify_one`, which is kept until this waits, so none is missed while
        // the worker is busy.
        let node_now = Node { inner };
        node_now.expire_short_links();
        let closing = node_now.inner.links_closing.load(Ordering::SeqCst);
        match node_now.next_link_job(closing) {
            Ok(job) => {
                if let Some(last) = last_request {
                    let pause = LINK_PACE.saturating_sub(last.elapsed());
                    if !pause.is_zero() {
                        drop(node_now);
                        tokio::time::sleep(pause).await;
                        continue;
                    }
                }
                let now = Instant::now();
                last_request = Some(now);
                #[cfg(test)]
                node_now.inner.link_state.lock().unwrap().sent.push(now);
                match job {
                    Job::Remove(code) => node_now.send_removal(code).await,
                    Job::Register(id) => node_now.send_registration(id).await,
                }
            }
            // Shutting down, nothing left to send. A removal waiting to be tried again is still
            // waited for (`stop_link_worker` bounds how long); whatever is left stays saved.
            Err(None) if closing => return,
            Err(wait) => {
                drop(node_now);
                match wait {
                    Some(wait) => {
                        tokio::select! {
                            _ = wake.notified() => {}
                            _ = tokio::time::sleep(wait) => {}
                        }
                    }
                    None => wake.notified().await,
                }
            }
        }
    }
}
