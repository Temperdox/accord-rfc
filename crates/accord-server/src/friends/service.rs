//! `FriendService` implementation: park friend requests for local users.
//!
//! A requester (often an `open_dms` guest from another home node) leaves a
//! request carrying their fr code; the recipient's client lists it, applies the
//! user's friend-request policy, and accepts or declines. Requests live in this
//! node's database so they survive restarts and wait for a logged-out recipient.
//! The server validates the code's shape (decodable, sane identity) for dedupe
//! and caps, but trust decisions (fingerprints, policy) stay client-side.

use std::sync::Arc;

use accord_proto::friend_service_server::FriendService;
use accord_proto::{
    DeleteFriendRequestRequest, DeleteFriendRequestResponse, FriendRequestEntry,
    GetPublicProfileRequest, GetPublicProfileResponse, ListFriendRequestsRequest,
    ListFriendRequestsResponse, SendFriendRequestRequest, SendFriendRequestResponse,
};
use accord_types::contact::ContactCode;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::error::ServerError;
use crate::store::Store;
use crate::util::authenticate;

/// Most requests parked per recipient (anti-spam; re-sends upsert, not grow).
const MAX_PARKED_REQUESTS: i64 = 50;
/// Longest accepted fr code (generous; codes are a few hundred bytes).
const MAX_CODE_LEN: usize = 8 * 1024;

/// Implements the `FriendService` RPCs.
#[derive(Debug)]
pub struct FriendSvc {
    store: Arc<dyn Store>,
    jwt: JwtKeys,
}

impl FriendSvc {
    /// Construct the service.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, jwt: JwtKeys) -> Self {
        Self { store, jwt }
    }

    fn caller<T>(&self, request: &Request<T>) -> Result<Uuid, ServerError> {
        let claims = authenticate(request, &self.jwt)?;
        Uuid::parse_str(&claims.sub)
            .map_err(|_| ServerError::InvalidArgument("invalid user id in token".into()))
    }
}

#[tonic::async_trait]
impl FriendService for FriendSvc {
    async fn send_friend_request(
        &self,
        request: Request<SendFriendRequestRequest>,
    ) -> Result<Response<SendFriendRequestResponse>, Status> {
        let _sender = self.caller(&request)?;
        let req = request.into_inner();

        let recipient = req
            .recipient
            .as_ref()
            .and_then(|u| Uuid::parse_str(&u.value).ok())
            .ok_or_else(|| ServerError::InvalidArgument("recipient id required".into()))?;
        if req.kind != "request" && req.kind != "accept" {
            return Err(
                ServerError::InvalidArgument("kind must be 'request' or 'accept'".into()).into(),
            );
        }
        if req.contact_code.len() > MAX_CODE_LEN {
            return Err(ServerError::InvalidArgument("contact code too large".into()).into());
        }
        // The code must decode and carry a sane identity key; the key doubles as
        // the dedupe handle so a sender can't park unlimited copies.
        let parsed = ContactCode::decode(&req.contact_code)
            .map_err(|e| ServerError::InvalidArgument(format!("bad contact code: {e}")))?;
        if parsed.identity_pubkey.len() != 32 {
            return Err(ServerError::InvalidArgument(
                "contact code has an invalid identity".into(),
            )
            .into());
        }

        if self.store.count_friend_requests(recipient).await? >= MAX_PARKED_REQUESTS {
            return Err(ServerError::RateLimited(
                "this user has too many pending friend requests".to_owned(),
            )
            .into());
        }
        self.store
            .upsert_friend_request(
                recipient,
                &parsed.identity_pubkey,
                &req.kind,
                &req.contact_code,
            )
            .await?;
        tracing::info!(%recipient, kind = %req.kind, "friend request parked");
        Ok(Response::new(SendFriendRequestResponse {}))
    }

    async fn list_friend_requests(
        &self,
        request: Request<ListFriendRequestsRequest>,
    ) -> Result<Response<ListFriendRequestsResponse>, Status> {
        let user_id = self.caller(&request)?;
        let rows = self.store.list_friend_requests(user_id).await?;
        Ok(Response::new(ListFriendRequestsResponse {
            requests: rows
                .into_iter()
                .map(|r| FriendRequestEntry {
                    id: r.id,
                    contact_code: r.contact_code,
                    kind: r.kind,
                    created_at_ms: r.created_at_ms,
                })
                .collect(),
        }))
    }

    async fn delete_friend_request(
        &self,
        request: Request<DeleteFriendRequestRequest>,
    ) -> Result<Response<DeleteFriendRequestResponse>, Status> {
        let user_id = self.caller(&request)?;
        let req = request.into_inner();
        self.store.delete_friend_request(user_id, &req.id).await?;
        Ok(Response::new(DeleteFriendRequestResponse {}))
    }

    async fn get_public_profile(
        &self,
        request: Request<GetPublicProfileRequest>,
    ) -> Result<Response<GetPublicProfileResponse>, Status> {
        // Authentication only - guests included. Being a guest here means the
        // profile owner shared their code; the data returned is what they
        // already put in it (kept live instead of frozen at code-gen time).
        let _caller = self.caller(&request)?;
        let req = request.into_inner();
        let user_id = req
            .user_id
            .as_ref()
            .and_then(|u| Uuid::parse_str(&u.value).ok())
            .ok_or_else(|| ServerError::InvalidArgument("user id required".into()))?;
        let (username, display_name) = self
            .store
            .user_profile(user_id)
            .await?
            .ok_or_else(|| ServerError::NotFound("no such user".to_owned()))?;
        Ok(Response::new(GetPublicProfileResponse {
            username,
            display_name,
        }))
    }
}
