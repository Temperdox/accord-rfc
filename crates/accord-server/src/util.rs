//! Small cross-cutting helpers shared by services.

use chrono::{DateTime, Utc};
use tonic::Request;

use crate::auth::jwt::{Claims, JwtKeys};
use crate::error::{ServerError, ServerResult};

/// Convert a chrono UTC timestamp into the protobuf well-known `Timestamp`.
#[must_use]
pub fn to_proto_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

/// Extract and verify the bearer token from a request's `authorization`
/// metadata, returning its claims.
///
/// Expected header form: `authorization: Bearer <jwt>`.
///
/// # Errors
/// Returns [`ServerError::Unauthenticated`] if the header is missing or the
/// token is invalid.
pub fn authenticate<T>(request: &Request<T>, keys: &JwtKeys) -> ServerResult<Claims> {
    let header = request
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(ServerError::Unauthenticated)?;

    let token = header.strip_prefix("Bearer ").unwrap_or(header);
    keys.verify(token)
}
