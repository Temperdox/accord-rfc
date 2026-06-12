//! gRPC helpers shared by the command handlers.

use std::time::Duration;

use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig};

use crate::state::SharedSessions;

/// Build a gRPC channel to `endpoint`, pinning `cert` for `https` (the server's
/// self-signed PEM from an invite/contact code) and adding keepalive pings so a
/// silently-dead connection is detected promptly.
///
/// # Errors
/// Returns an error string if the endpoint is invalid, TLS config fails, or the
/// connection cannot be established.
pub async fn build_channel(endpoint: &str, cert: Option<&str>) -> Result<Channel, String> {
    let mut ep = Channel::from_shared(endpoint.to_owned())
        .map_err(|e| format!("invalid endpoint: {e}"))?
        .http2_keep_alive_interval(Duration::from_secs(20))
        .keep_alive_timeout(Duration::from_secs(20))
        .keep_alive_while_idle(true)
        // Fail fast on an unreachable address so callers can fall back to the
        // next one (e.g. LAN -> mesh).
        .connect_timeout(Duration::from_secs(8));

    if endpoint.starts_with("https") {
        let tls = match cert {
            Some(pem) => ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(pem))
                .domain_name(accord_server::tls::PINNED_DOMAIN),
            None => ClientTlsConfig::new(),
        };
        ep = ep.tls_config(tls).map_err(|e| format!("tls config: {e}"))?;
    }

    ep.connect()
        .await
        .map_err(|e| format!("could not connect: {e}"))
}

/// Try each `http(s)` address in order, returning the first that connects (its
/// endpoint + channel). This is how a DM reaches a contact: their LAN address on
/// the same network, falling back to their mesh address across the internet.
///
/// # Errors
/// Returns an error if no address is reachable.
pub async fn connect_first(
    addresses: &[String],
    cert: Option<&str>,
) -> Result<(String, Channel), String> {
    let mut tried = 0;
    let mut last = String::new();
    for ep in addresses.iter().filter(|a| a.starts_with("http")) {
        tried += 1;
        match build_channel(ep, cert).await {
            Ok(channel) => return Ok((ep.clone(), channel)),
            Err(e) => last = e,
        }
    }
    if tried == 0 {
        return Err("this contact has no reachable address yet".to_owned());
    }
    Err(format!(
        "could not reach this contact (are you both online; is the mesh running?): {last}"
    ))
}

/// Attach `authorization: Bearer <token>` metadata to a request.
///
/// # Errors
/// Returns an error string if the token cannot be encoded as metadata.
pub fn authed<T>(mut req: Request<T>, token: &str) -> Result<Request<T>, String> {
    let value: MetadataValue<_> = format!("Bearer {token}")
        .parse()
        .map_err(|_| "invalid token".to_string())?;
    req.metadata_mut().insert("authorization", value);
    Ok(req)
}

/// Convert a gRPC status into a user-facing error string.
#[must_use]
pub fn status_to_string(status: tonic::Status) -> String {
    format!("{}: {}", status.code(), status.message())
}

/// Clone the active session's channel, or error if not connected.
///
/// # Errors
/// Returns an error string if there is no active connection.
pub async fn require_channel(state: &SharedSessions) -> Result<Channel, String> {
    state
        .lock()
        .await
        .active_channel()
        .ok_or_else(|| "not connected to a server".to_string())
}

/// Clone the active session's channel and access token, or error if not logged in.
///
/// # Errors
/// Returns an error string if there is no active connection or it isn't logged in.
pub async fn require_session(state: &SharedSessions) -> Result<(Channel, String), String> {
    state
        .lock()
        .await
        .active_channel_token()
        .ok_or_else(|| "not connected / not logged in".to_string())
}
