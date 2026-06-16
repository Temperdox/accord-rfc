//! Guardrails / auto-mod — abuse resistance ORTHOGONAL to RBAC.
//!
//! RBAC ([`crate::authz`]) answers *"may you do this at all?"*. Guardrails answer
//! *"how fast, and does this look hostile?"* — and they apply **even to
//! ADMINISTRATOR roles**, so a compromised or rogue admin can't mass-delete
//! channels, mass-kick, or flood new channels faster than the limits allow. A
//! tripped guardrail is recorded to the audit log and broadcast as a live
//! [`ModAlert`](accord_proto::ModAlert) to the owner/other admins (wired in the
//! service + hub), so hostile attempts are witnessed in real time.
//!
//! This is the shift-left scaffold for the abuse model in `BAN-PLAN.md` /
//! `BOT-API-PLAN.md`: the enforcement points and decision types exist now; the
//! heavier cryptographic layers (ban-tag PRF, proof-of-work) layer on later.
//!
//! Owner policy: by default the owner is *audited + alerted* but not hard-blocked
//! (`block_owner = false`) — the owner is the root of trust and can disable
//! guardrails anyway. Flip [`GuardrailConfig::block_owner`] to subject the owner
//! to the same limits.
//!
//! State is in-memory and per-instance (token buckets keyed by `(actor, class)`).
//! That's the right scope for single-instance self-hosting; a cross-instance
//! limiter (Redis) is a later concern, same as the hub's voice registry.

mod names;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use uuid::Uuid;

pub use names::{NameVerdict, assess_name};

/// A privileged action subject to guardrails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionClass {
    CreateChannel,
    DeleteChannel,
    KickMember,
    BanMember,
    CreateRole,
    UpdateRole,
    DeleteRole,
    AssignRole,
    UpdateServer,
}

impl ActionClass {
    /// Wire/audit string for this action (also the `ModAlert.action` field).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ActionClass::CreateChannel => "create_channel",
            ActionClass::DeleteChannel => "delete_channel",
            ActionClass::KickMember => "kick_member",
            ActionClass::BanMember => "ban_member",
            ActionClass::CreateRole => "create_role",
            ActionClass::UpdateRole => "update_role",
            ActionClass::DeleteRole => "delete_role",
            ActionClass::AssignRole => "assign_role",
            ActionClass::UpdateServer => "update_server",
        }
    }

    /// Destructive actions get tight budgets; additive ones get generous budgets.
    #[must_use]
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            ActionClass::DeleteChannel
                | ActionClass::KickMember
                | ActionClass::BanMember
                | ActionClass::DeleteRole
        )
    }

    /// `(capacity, refill_tokens_per_sec)` for this action's token bucket.
    fn budget(self) -> (f64, f64) {
        match self {
            // Destructive: small burst, slow refill (1 per 20s).
            ActionClass::DeleteChannel
            | ActionClass::KickMember
            | ActionClass::BanMember
            | ActionClass::DeleteRole => (3.0, 1.0 / 20.0),
            // Role/server mutations: moderate.
            ActionClass::UpdateRole | ActionClass::AssignRole | ActionClass::UpdateServer => {
                (5.0, 1.0 / 5.0)
            }
            // Additive: generous (1 per 5s, burst of 5).
            ActionClass::CreateChannel | ActionClass::CreateRole => (5.0, 1.0 / 5.0),
        }
    }
}

/// The outcome of a guardrail check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardrailDecision {
    /// Proceed normally.
    Allow,
    /// Proceed, but the action is suspicious — record + alert admins.
    AllowFlagged { reason: String },
    /// Rate-limited; retry after roughly this many seconds.
    Throttle { retry_after_secs: u64, reason: String },
    /// Refused outright.
    Deny { reason: String },
}

impl GuardrailDecision {
    /// Whether the caller should still perform the action.
    #[must_use]
    pub fn allowed(&self) -> bool {
        matches!(self, GuardrailDecision::Allow | GuardrailDecision::AllowFlagged { .. })
    }

    /// Whether this decision warrants an audit row + `ModAlert`.
    #[must_use]
    pub fn is_notable(&self) -> bool {
        !matches!(self, GuardrailDecision::Allow)
    }
}

/// Context for a single guardrail check.
#[derive(Debug, Default, Clone)]
pub struct ActionContext<'a> {
    /// Proposed channel name (for `CreateChannel` name heuristics).
    pub name: Option<&'a str>,
    /// Recent channel names, for low-variance/spam detection.
    pub recent_names: &'a [String],
    /// Whether the actor is the server owner.
    pub is_owner: bool,
}

/// Tunable guardrail policy.
#[derive(Debug, Clone)]
pub struct GuardrailConfig {
    /// Subject the owner to rate limits too (default false: owner is alerted but
    /// not blocked).
    pub block_owner: bool,
}

impl Default for GuardrailConfig {
    fn default() -> Self {
        Self { block_owner: false }
    }
}

/// A leaky/token bucket: `tokens` accrue at `refill` per second up to `capacity`.
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    capacity: f64,
    refill: f64,
    last: Instant,
}

impl TokenBucket {
    fn new(capacity: f64, refill: f64, now: Instant) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill,
            last: now,
        }
    }

    /// Try to spend one token. On success returns `None`; on failure returns the
    /// approximate seconds until one token is available.
    fn try_take(&mut self, now: Instant) -> Option<u64> {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None
        } else {
            let deficit = 1.0 - self.tokens;
            let secs = (deficit / self.refill).ceil() as u64;
            Some(secs.max(1))
        }
    }
}

/// The guardrail engine. Cheap to clone-share behind an `Arc`.
#[derive(Debug)]
pub struct Guardrails {
    config: GuardrailConfig,
    buckets: Mutex<HashMap<(Uuid, ActionClass), TokenBucket>>,
}

impl Default for Guardrails {
    fn default() -> Self {
        Self::new(GuardrailConfig::default())
    }
}

impl Guardrails {
    #[must_use]
    pub fn new(config: GuardrailConfig) -> Self {
        Self {
            config,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Evaluate an action. Pure in-memory; the caller maps the decision onto a
    /// gRPC status and (when notable) records the audit row + emits a `ModAlert`.
    #[must_use]
    pub fn check(&self, actor: Uuid, action: ActionClass, ctx: &ActionContext) -> GuardrailDecision {
        self.check_at(actor, action, ctx, Instant::now())
    }

    /// [`check`](Self::check) with an injectable clock (for tests).
    #[must_use]
    pub fn check_at(
        &self,
        actor: Uuid,
        action: ActionClass,
        ctx: &ActionContext,
        now: Instant,
    ) -> GuardrailDecision {
        // Name heuristics first (additive griefing): a spammy/random channel name
        // is flagged even when within rate — alert admins, but let it through (a
        // hard block on names risks false positives; the rate limit is the wall).
        let mut flagged: Option<String> = None;
        if action == ActionClass::CreateChannel {
            if let Some(name) = ctx.name {
                if let NameVerdict::Suspicious { reason } = assess_name(name, ctx.recent_names) {
                    flagged = Some(reason);
                }
            }
        }

        // Rate limit (skipped for the owner unless block_owner). Applies to admins.
        let throttle = if ctx.is_owner && !self.config.block_owner {
            None
        } else {
            let (capacity, refill) = action.budget();
            let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
            let bucket = buckets
                .entry((actor, action))
                .or_insert_with(|| TokenBucket::new(capacity, refill, now));
            bucket.try_take(now)
        };

        if let Some(retry_after_secs) = throttle {
            let kind = if action.is_destructive() {
                "destructive"
            } else {
                "additive"
            };
            return GuardrailDecision::Throttle {
                retry_after_secs,
                reason: format!(
                    "{} action '{}' rate-limited; retry in ~{}s",
                    kind,
                    action.as_str(),
                    retry_after_secs
                ),
            };
        }

        match flagged {
            Some(reason) => GuardrailDecision::AllowFlagged { reason },
            None => GuardrailDecision::Allow,
        }
    }
}
