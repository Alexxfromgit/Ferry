//! Client of the short-link service (`docs/LINKS.md`): the sender registers
//! `CODE → (device, token, home relay)`, and recipients look codes up.
//!
//! The service is only a phone book. What it answers is checked: the code must be the one
//! derived from the device and token it returns, so it can't point a link at another device,
//! and the connection to that device is authenticated by its key anyway. Registrations are
//! signed with the device key, so nobody else can register or remove this device's links.

use std::time::Duration;

use anyhow::{Context, bail, ensure};
use iroh::{EndpointId, RelayUrl, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::link::{ShareToken, share_code};

/// Where Ferry's own service runs.
pub const DEFAULT_LINK_SERVICE: &str = "https://ferry.xenvaultinteractive.com";
/// How long a registration lasts; Ferry refreshes it long before that while a share exists.
pub(crate) const LINK_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Answers are a few hundred bytes.
const MAX_ANSWER: usize = 16 * 1024;
const REGISTER_CONTEXT: &[u8] = b"ferry-link-v1";
const DELETE_CONTEXT: &[u8] = b"ferry-link-del-v1";

/// What a code stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub endpoint: EndpointId,
    pub token: ShareToken,
    pub relay: Option<RelayUrl>,
}

#[derive(Clone)]
pub(crate) struct LinkService {
    base: String,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct RegisterBody<'a> {
    endpoint: String,
    token: String,
    relay: Option<&'a str>,
    expires: u64,
    sig: String,
}

#[derive(Serialize)]
struct DeleteBody {
    endpoint: String,
    timestamp: u64,
    sig: String,
}

#[derive(Deserialize)]
struct LookupAnswer {
    endpoint: String,
    token: String,
    relay: Option<String>,
    /// The registration's own expiry and signature, so the relay can be checked too.
    #[serde(default)]
    expires: Option<u64>,
    #[serde(default)]
    sig: Option<String>,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    error: String,
}

impl LinkService {
    /// A client for the service at `base` (`https://host`, or `http://` for tests), using
    /// `tls` (the endpoint's TLS settings) for HTTPS.
    pub fn new(base: &str, tls: rustls::ClientConfig) -> anyhow::Result<Self> {
        let base = base.trim_end_matches('/').to_string();
        ensure!(
            base.starts_with("https://") || base.starts_with("http://"),
            "the link service address must start with https://"
        );
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Ferry/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("setting up the link service client")?;
        Ok(Self { base, http })
    }

    fn url(&self, code: &str) -> String {
        format!("{}/api/links/{code}", self.base)
    }

    /// Register (or refresh) the short code of the share named by `token`, pointing at this
    /// device. Returns the code.
    pub async fn register(
        &self,
        secret: &SecretKey,
        token: &ShareToken,
        relay: Option<&RelayUrl>,
        expires: u64,
    ) -> anyhow::Result<String> {
        let endpoint = secret.public();
        let code = share_code(&endpoint, token);
        // The service takes https relays only; any other is left out rather than failing.
        let relay = relay
            .filter(|r| r.scheme() == "https")
            .map(|r| r.to_string());
        let message = register_message(&code, &endpoint, token, relay.as_deref(), expires);
        let body = RegisterBody {
            endpoint: hex::encode(endpoint.as_bytes()),
            token: hex::encode(token.0),
            relay: relay.as_deref(),
            expires,
            sig: hex::encode(secret.sign(&message).to_bytes()),
        };
        let response = self
            .http
            .put(self.url(&code))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body)?)
            .send()
            .await
            // Errors would name the URL, which holds the code; keep that out of logs.
            .map_err(reqwest::Error::without_url)
            .context("could not reach the link service")?;
        check(response).await?;
        Ok(code)
    }

    /// Remove a code of this device's. Removing one that is already gone succeeds.
    pub async fn delete(&self, secret: &SecretKey, code: &str, now: u64) -> anyhow::Result<()> {
        let endpoint = secret.public();
        let message = delete_message(code, &endpoint, now);
        let body = DeleteBody {
            endpoint: hex::encode(endpoint.as_bytes()),
            timestamp: now,
            sig: hex::encode(secret.sign(&message).to_bytes()),
        };
        let response = self
            .http
            .delete(self.url(code))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body)?)
            .send()
            .await
            // Errors would name the URL, which holds the code; keep that out of logs.
            .map_err(reqwest::Error::without_url)
            .context("could not reach the link service")?;
        check(response).await?;
        Ok(())
    }

    /// Look a code up, and check that the answer really is the share the code names.
    pub async fn resolve(&self, code: &str) -> anyhow::Result<Resolved> {
        let response = self
            .http
            .get(self.url(code))
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("could not reach the link service to look up this short link")?;
        let body = check(response).await?;
        let answer: LookupAnswer =
            serde_json::from_slice(&body).context("the link service's answer is not understood")?;
        let endpoint_bytes: [u8; 32] = hex::decode(&answer.endpoint)
            .ok()
            .and_then(|b| b.try_into().ok())
            .context("the link service's answer is damaged")?;
        let endpoint = EndpointId::from_bytes(&endpoint_bytes)
            .ok()
            .context("the link service's answer is damaged")?;
        let token = ShareToken(
            hex::decode(&answer.token)
                .ok()
                .and_then(|b| b.try_into().ok())
                .context("the link service's answer is damaged")?,
        );
        ensure!(
            share_code(&endpoint, &token) == code,
            "the link service gave an answer that doesn't belong to this link"
        );
        // The relay is used only if the device signed it, in a registration that still lasts:
        // otherwise the service could steer recipients to a relay of its choosing, or to one
        // the device stopped using. Without it, the device is still found through address
        // lookup.
        let signed = match (&answer.sig, answer.expires) {
            (Some(sig), Some(expires)) if expires > crate::util::now_unix() => {
                let message =
                    register_message(code, &endpoint, &token, answer.relay.as_deref(), expires);
                hex::decode(sig)
                    .ok()
                    .and_then(|b| <[u8; 64]>::try_from(b).ok())
                    .is_some_and(|b| {
                        endpoint
                            .verify(&message, &Signature::from_bytes(&b))
                            .is_ok()
                    })
            }
            _ => false,
        };
        let relay = if signed {
            answer.relay.and_then(|r| r.parse().ok())
        } else {
            None
        };
        Ok(Resolved {
            endpoint,
            token,
            relay,
        })
    }
}

/// The body of a successful answer, or an error saying what the service said.
async fn check(mut response: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    let status = response.status();
    let too_large = "the link service's answer is too large";
    ensure!(
        response.content_length().unwrap_or(0) <= MAX_ANSWER as u64,
        too_large
    );
    // Read no more than the limit, whatever the answer claims.
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)
        .context("the link service's answer was cut off")?
    {
        ensure!(body.len() + chunk.len() <= MAX_ANSWER, too_large);
        body.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return Ok(body);
    }
    let said = serde_json::from_slice::<ErrorAnswer>(&body)
        .map(|e| e.error)
        .unwrap_or_default();
    match status.as_u16() {
        404 => bail!("this short link doesn't exist or has expired; ask the sender for a new one"),
        429 => {
            bail!("too many short links were looked up from this network; try again in a minute")
        }
        code => bail!(
            "the link service answered {code}{}",
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        ),
    }
}

/// What a registration signs: `"ferry-link-v1" ‖ CODE ‖ endpoint ‖ token ‖ u16 relay length ‖
/// relay ‖ u64 expires`, integers big-endian.
fn register_message(
    code: &str,
    endpoint: &EndpointId,
    token: &ShareToken,
    relay: Option<&str>,
    expires: u64,
) -> Vec<u8> {
    let relay = relay.unwrap_or_default().as_bytes();
    let mut message = Vec::with_capacity(128 + relay.len());
    message.extend_from_slice(REGISTER_CONTEXT);
    message.extend_from_slice(code.as_bytes());
    message.extend_from_slice(endpoint.as_bytes());
    message.extend_from_slice(&token.0);
    message.extend_from_slice(&(relay.len() as u16).to_be_bytes());
    message.extend_from_slice(relay);
    message.extend_from_slice(&expires.to_be_bytes());
    message
}

/// What a removal signs: `"ferry-link-del-v1" ‖ CODE ‖ endpoint ‖ u64 timestamp`.
fn delete_message(code: &str, endpoint: &EndpointId, timestamp: u64) -> Vec<u8> {
    let mut message = Vec::with_capacity(96);
    message.extend_from_slice(DELETE_CONTEXT);
    message.extend_from_slice(code.as_bytes());
    message.extend_from_slice(endpoint.as_bytes());
    message.extend_from_slice(&timestamp.to_be_bytes());
    message
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use iroh::Signature;
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };

    use super::*;

    /// A stand-in for the link service, following `docs/LINKS.md`: it checks codes and
    /// signatures like the real one. `lie` makes lookups answer with another share.
    #[derive(Clone, Default)]
    pub struct MockService {
        pub links: Arc<Mutex<HashMap<String, serde_json::Value>>>,
        pub lie: Arc<Mutex<bool>>,
        /// Answer lookups with this relay instead of the signed one.
        pub swap_relay: Arc<Mutex<Option<String>>>,
        /// Answer this many registrations and removals with 503 first.
        pub fail_writes: Arc<Mutex<usize>>,
        /// Answer every registration and removal with 503 (the service is down).
        pub down: Arc<Mutex<bool>>,
        /// Codes whose registration is always refused (409).
        pub refuse: Arc<Mutex<std::collections::HashSet<String>>>,
        /// When each registration or removal arrived.
        pub writes: Arc<Mutex<Vec<std::time::Instant>>>,
    }

    impl MockService {
        /// Serve on localhost; returns the base URL.
        pub async fn start(&self) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let service = self.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let service = service.clone();
                    tokio::spawn(async move {
                        let (read, mut write) = stream.into_split();
                        let mut read = BufReader::new(read);
                        let mut request_line = String::new();
                        read.read_line(&mut request_line).await.ok();
                        let mut length = 0;
                        loop {
                            let mut line = String::new();
                            read.read_line(&mut line).await.ok();
                            let line = line.trim_end();
                            if line.is_empty() {
                                break;
                            }
                            if let Some((name, value)) = line.split_once(':')
                                && name.eq_ignore_ascii_case("content-length")
                            {
                                length = value.trim().parse().unwrap_or(0);
                            }
                        }
                        let mut body = vec![0; length];
                        read.read_exact(&mut body).await.ok();
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().unwrap_or_default().to_string();
                        let path = parts.next().unwrap_or_default().to_string();
                        let (status, answer) = service.answer(&method, &path, &body);
                        let response = format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                            answer.len()
                        );
                        write.write_all(response.as_bytes()).await.ok();
                    });
                }
            });
            base
        }

        fn answer(&self, method: &str, path: &str, body: &[u8]) -> (u16, String) {
            let Some(code) = path.strip_prefix("/api/links/") else {
                return (404, r#"{"error":"Not found."}"#.into());
            };
            let code = code.to_ascii_uppercase();
            let body: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let bytes = |key: &str| hex::decode(body[key].as_str().unwrap_or_default()).ok();
            let endpoint = bytes("endpoint")
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
                .and_then(|b| EndpointId::from_bytes(&b).ok());
            let sig = bytes("sig")
                .and_then(|b| <[u8; 64]>::try_from(b).ok())
                .map(|b| Signature::from_bytes(&b));
            if method != "GET" {
                self.writes.lock().unwrap().push(std::time::Instant::now());
                let mut fail = self.fail_writes.lock().unwrap();
                if *self.down.lock().unwrap() || *fail > 0 {
                    *fail = fail.saturating_sub(1);
                    return (503, r#"{"error":"down"}"#.into());
                }
                if method == "PUT" && self.refuse.lock().unwrap().contains(&code) {
                    return (409, r#"{"error":"taken"}"#.into());
                }
            }
            match method {
                "GET" => match self.links.lock().unwrap().get(&code) {
                    Some(link) if *self.lie.lock().unwrap() => {
                        let mut link = link.clone();
                        link["token"] = hex::encode(ShareToken::generate().0).into();
                        (200, link.to_string())
                    }
                    Some(link) => {
                        let mut link = link.clone();
                        if let Some(relay) = self.swap_relay.lock().unwrap().clone() {
                            link["relay"] = relay.into();
                        }
                        (200, link.to_string())
                    }
                    None => (
                        404,
                        r#"{"error":"This link doesn't exist or has expired."}"#.into(),
                    ),
                },
                "PUT" => {
                    let (Some(endpoint), Some(sig)) = (endpoint, sig) else {
                        return (400, r#"{"error":"bad body"}"#.into());
                    };
                    let token = ShareToken(
                        bytes("token")
                            .and_then(|b| b.try_into().ok())
                            .unwrap_or_default(),
                    );
                    if share_code(&endpoint, &token) != code {
                        return (400, r#"{"error":"code mismatch"}"#.into());
                    }
                    let relay = body["relay"].as_str();
                    let expires = body["expires"].as_u64().unwrap_or_default();
                    let message = register_message(&code, &endpoint, &token, relay, expires);
                    if endpoint.verify(&message, &sig).is_err() {
                        return (403, r#"{"error":"The signature doesn't verify."}"#.into());
                    }
                    let link = serde_json::json!({
                        "endpoint": body["endpoint"], "token": body["token"], "relay": body["relay"],
                        "expires": body["expires"], "sig": body["sig"],
                    });
                    self.links.lock().unwrap().insert(code, link);
                    (204, String::new())
                }
                "DELETE" => {
                    let (Some(endpoint), Some(sig)) = (endpoint, sig) else {
                        return (400, r#"{"error":"bad body"}"#.into());
                    };
                    let timestamp = body["timestamp"].as_u64().unwrap_or_default();
                    let message = delete_message(&code, &endpoint, timestamp);
                    if endpoint.verify(&message, &sig).is_err() {
                        return (403, r#"{"error":"The signature doesn't verify."}"#.into());
                    }
                    self.links.lock().unwrap().remove(&code);
                    (204, String::new())
                }
                _ => (405, r#"{"error":"method"}"#.into()),
            }
        }
    }

    /// A client set up the way a node sets it up, with its endpoint's TLS settings.
    async fn client(base: &str) -> LinkService {
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .unwrap();
        LinkService::new(base, endpoint.tls_config().clone()).unwrap()
    }

    #[test]
    fn signed_messages_follow_the_documented_layout() {
        let endpoint: [u8; 32] = std::array::from_fn(|i| i as u8 + 1);
        let key = SecretKey::from_bytes(&endpoint);
        let id = key.public();
        let token = ShareToken(std::array::from_fn(|i| 0xA0 + i as u8));
        let message = register_message(
            "6N76Y6BE0EQX",
            &id,
            &token,
            Some("https://relay.example"),
            0x0102030405,
        );
        let expected = [
            b"ferry-link-v1".as_slice(),
            b"6N76Y6BE0EQX",
            id.as_bytes(),
            &token.0,
            &[0, 21],
            b"https://relay.example",
            &[0, 0, 0, 1, 2, 3, 4, 5],
        ]
        .concat();
        assert_eq!(message, expected);
        let without_relay = register_message("6N76Y6BE0EQX", &id, &token, None, 1);
        assert_eq!(without_relay[13 + 12 + 32 + 16..][..2], [0, 0]);
        assert_eq!(
            delete_message("6N76Y6BE0EQX", &id, 7),
            [
                b"ferry-link-del-v1".as_slice(),
                b"6N76Y6BE0EQX",
                id.as_bytes(),
                &7u64.to_be_bytes()
            ]
            .concat()
        );
    }

    #[tokio::test]
    async fn register_resolve_and_delete() {
        let mock = MockService::default();
        let service = client(&mock.start().await).await;
        let secret = SecretKey::generate();
        let token = ShareToken::generate();
        let relay: RelayUrl = "https://relay.example.com".parse().unwrap();
        let code = service
            .register(&secret, &token, Some(&relay), 4_000_000_000)
            .await
            .unwrap();
        assert_eq!(code, share_code(&secret.public(), &token));

        let resolved = service.resolve(&code).await.unwrap();
        assert_eq!(
            resolved,
            Resolved {
                endpoint: secret.public(),
                token,
                relay: Some(relay.clone())
            }
        );

        // A relay the device didn't sign is left out.
        *mock.swap_relay.lock().unwrap() = Some("https://evil.example.com/".into());
        assert_eq!(service.resolve(&code).await.unwrap().relay, None);
        *mock.swap_relay.lock().unwrap() = None;

        // A relay from a registration that ran out is left out too, though its signature holds.
        service
            .register(&secret, &token, Some(&relay), 1_000)
            .await
            .unwrap();
        assert_eq!(service.resolve(&code).await.unwrap().relay, None);

        // A relay other than https is left out of the registration, not refused.
        let http_relay: RelayUrl = "http://10.0.0.5:3340".parse().unwrap();
        service
            .register(&secret, &token, Some(&http_relay), 4_000_000_000)
            .await
            .unwrap();
        assert_eq!(service.resolve(&code).await.unwrap().relay, None);

        // A service that points the code at something else is caught.
        *mock.lie.lock().unwrap() = true;
        let err = service.resolve(&code).await.unwrap_err().to_string();
        assert!(err.contains("doesn't belong"), "{err}");
        *mock.lie.lock().unwrap() = false;

        service.delete(&secret, &code, 1).await.unwrap();
        let err = service.resolve(&code).await.unwrap_err().to_string();
        assert!(err.contains("doesn't exist"), "{err}");
    }
}
