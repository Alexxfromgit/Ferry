//! The Ferry protocol (`ferry/1`): a recipient presents a share's token, and the password if
//! the share is locked; the sender answers with the share's content hash and lets that device
//! fetch it over `iroh-blobs`.
//!
//! One request and one answer per bidirectional stream, each a postcard message ending with
//! the stream. The connection is authenticated by iroh (the recipient dialed the sender's
//! device key), so a password sent here reaches only the sender.

use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Context;
use iroh::{
    EndpointId,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_blobs::Hash;
use serde::{Deserialize, Serialize};

use crate::link::ShareToken;

pub const ALPN: &[u8] = b"ferry/1";
/// Requests and answers are tiny; anything larger is not a Ferry peer.
const MAX_MESSAGE: usize = 4 * 1024;
// The longest password a share takes must fit in one request, with the token and framing.
const _: () = assert!(crate::access::MAX_PASSWORD_BYTES + 256 <= MAX_MESSAGE);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Requests one connection may make before it is closed (each guess needs a new one).
const MAX_REQUESTS_PER_CONNECTION: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Open(OpenRequest),
}

#[derive(Serialize, Deserialize)]
pub struct OpenRequest {
    pub token: ShareToken,
    pub password: Option<String>,
}

impl std::fmt::Debug for OpenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRequest")
            .field("password", &self.password.as_ref().map(|_| ".."))
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenResponse {
    /// The device may fetch the share with this root hash. `lan` says where the sender can be
    /// reached directly on a local network, if it might be (see [`LanAddr`]).
    Ok { hash: Hash, lan: Option<LanAddr> },
    /// The share is locked and no password was given.
    PasswordRequired,
    /// The password is wrong, or too many guesses were made; try again after the delay.
    WrongPassword { retry_after_secs: u64 },
    /// No such share, or sharing it is turned off.
    Unavailable,
}

/// Where a sender can be reached on a local network: an endpoint and its local (private)
/// addresses. Devices elsewhere can't reach those, and they say nothing about where anyone is,
/// so the sender can hand them to anyone it lets in, even while it hides its IP address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanAddr {
    pub id: EndpointId,
    pub addrs: Vec<std::net::SocketAddr>,
}

/// Local addresses offered at most.
pub const MAX_LAN_ADDRS: usize = 8;

/// Whether `ip` is a local-network address worth offering: private IPv4 ranges, the shared
/// carrier and VPN range 100.64/10, and link-local; loopback only when `loopback` is set.
pub fn is_lan_ip(ip: std::net::IpAddr, loopback: bool) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private()
                || v4.is_link_local()
                || (a == 100 && (64..128).contains(&b))
                || (loopback && v4.is_loopback())
        }
        // Local-network IPv6 addresses are usually global ones, which would give the location
        // away; only IPv4 is offered.
        std::net::IpAddr::V6(_) => false,
    }
}

/// What the sender does with requests.
pub trait Handler: Send + Sync + 'static {
    fn open(
        &self,
        remote: EndpointId,
        request: OpenRequest,
    ) -> impl Future<Output = OpenResponse> + Send;
}

/// Serves `ferry/1` on a router.
pub struct Protocol<H> {
    handler: Arc<H>,
}

impl<H> Protocol<H> {
    pub fn new(handler: Arc<H>) -> Self {
        Self { handler }
    }
}

impl<H> std::fmt::Debug for Protocol<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ferry::Protocol")
    }
}

impl<H: Handler> ProtocolHandler for Protocol<H> {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote = connection.remote_id();
        for _ in 0..MAX_REQUESTS_PER_CONNECTION {
            let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                return Ok(());
            };
            let answer = tokio::time::timeout(REQUEST_TIMEOUT, async {
                let bytes = recv.read_to_end(MAX_MESSAGE).await.ok()?;
                let request: Request = postcard::from_bytes(&bytes).ok()?;
                Some(match request {
                    Request::Open(open) => self.handler.open(remote, open).await,
                })
            })
            .await;
            let Ok(Some(answer)) = answer else {
                connection.close(1u32.into(), b"bad request");
                return Ok(());
            };
            let bytes = postcard::to_stdvec(&answer).map_err(AcceptError::from_err)?;
            send.write_all(&bytes)
                .await
                .map_err(AcceptError::from_err)?;
            send.finish()?;
        }
        // Let the last answer arrive before the connection is dropped.
        tokio::time::timeout(Duration::from_secs(5), connection.closed())
            .await
            .ok();
        Ok(())
    }
}

/// Ask the sender on `connection` for a share.
pub async fn open(connection: &Connection, request: OpenRequest) -> anyhow::Result<OpenResponse> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("could not talk to the sender")?;
    let bytes = postcard::to_stdvec(&Request::Open(request))?;
    send.write_all(&bytes)
        .await
        .context("could not talk to the sender")?;
    send.finish()?;
    let bytes = tokio::time::timeout(REQUEST_TIMEOUT, recv.read_to_end(MAX_MESSAGE))
        .await
        .context("the sender did not answer")?
        .context("the sender did not answer")?;
    postcard::from_bytes(&bytes).context("the sender's answer is not understood; update Ferry")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_addresses_are_offered() {
        let lan = |ip: &str, loopback| is_lan_ip(ip.parse().unwrap(), loopback);
        for local in [
            "192.168.1.20",
            "10.0.0.5",
            "172.16.3.4",
            "169.254.1.1",
            "100.64.0.1",
        ] {
            assert!(lan(local, false), "{local}");
        }
        for not_local in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "127.0.0.1",
            "fe80::1",
            "fd00::1",
            "2001:db8::1",
        ] {
            assert!(!lan(not_local, false), "{not_local}");
        }
        assert!(lan("127.0.0.1", true), "loopback only in tests");
    }
}
