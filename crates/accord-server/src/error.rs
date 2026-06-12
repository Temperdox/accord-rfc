//! Server error type and its mapping to gRPC [`tonic::Status`].
//!
//! Handlers return [`ServerError`] internally; the [`From`] impl converts it to
//! a `Status` at the gRPC boundary. We deliberately map internal failures
//! (database, redis) to `Status::internal` with a generic message so we never
//! leak implementation detail to clients, while logging the real cause.

use tonic::{Code, Status};

/// All error conditions the server's business logic can produce.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Credentials were wrong, or the token was missing/invalid/expired.
    #[error("authentication failed")]
    Unauthenticated,

    /// The caller is authenticated but not allowed to perform the action.
    #[error("permission denied")]
    PermissionDenied,

    /// A uniqueness constraint was violated (e.g. username already taken).
    #[error("already exists: {0}")]
    AlreadyExists(String),

    /// A requested entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The request was malformed (bad ID, empty field, ...).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The caller exceeded a rate limit (e.g. registrations per IP).
    #[error("rate limited: {0}")]
    RateLimited(String),

    /// A database error. Mapped to `internal`; details are logged, not returned.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// A schema migration failed at startup.
    #[error("migration error: {0}")]
    Migration(String),

    /// An internal invariant was violated (e.g. a stored id failed to parse).
    #[error("internal error: {0}")]
    Internal(String),

    /// A Redis error. Mapped to `internal`.
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),

    /// Password hashing/verification failure (other than a simple mismatch).
    #[error("password hashing error: {0}")]
    PasswordHash(String),

    /// JWT signing/verification failure.
    #[error("token error: {0}")]
    Token(String),
}

/// Convenience alias for handler results.
pub type ServerResult<T> = Result<T, ServerError>;

impl From<ServerError> for Status {
    fn from(err: ServerError) -> Self {
        // Log the full internal error, but only surface a safe message/code.
        match &err {
            ServerError::Unauthenticated => Status::new(Code::Unauthenticated, err.to_string()),
            ServerError::PermissionDenied => Status::new(Code::PermissionDenied, err.to_string()),
            ServerError::AlreadyExists(_) => Status::new(Code::AlreadyExists, err.to_string()),
            ServerError::NotFound(_) => Status::new(Code::NotFound, err.to_string()),
            ServerError::InvalidArgument(_) => Status::new(Code::InvalidArgument, err.to_string()),
            ServerError::RateLimited(_) => Status::new(Code::ResourceExhausted, err.to_string()),
            ServerError::Database(_)
            | ServerError::Redis(_)
            | ServerError::Migration(_)
            | ServerError::Internal(_)
            | ServerError::PasswordHash(_)
            | ServerError::Token(_) => {
                tracing::error!(error = %err, "internal server error");
                Status::new(Code::Internal, "internal server error")
            }
        }
    }
}
