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
    /// The device may fetch the share with this root hash.
    Ok { hash: Hash },
    /// The share is locked and no password was given.
    PasswordRequired,
    /// The password is wrong, or too many guesses were made; try again after the delay.
    WrongPassword { retry_after_secs: u64 },
    /// No such share, or sharing it is turned off.
    Unavailable,
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
