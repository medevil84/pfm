//! Impersonation service: start and stop an impersonation session.
//!
//! The impersonation state is stored in the HTTP session (cookie), not in the
//! database. The service handles the audit trail: every start/stop writes an
//! audit row with `actor_user_id = the real admin` and
//! `impersonated_as_user_id = the user being impersonated`.

use pfm_domain::{AuditAction, AuditEvent};
use pfm_repo::{AuditRepo, SqlxAuthRepo, UsersRepo};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImpersonationError {
    #[error("user not found")]
    UserNotFound,
    #[error("target user is not active")]
    TargetInactive,
    #[error("not currently impersonating")]
    NotImpersonating,
    #[error("internal error: {0}")]
    Internal(String),
}

/// Service for starting and stopping impersonation sessions.
pub struct ImpersonationService {
    repo: SqlxAuthRepo,
}

impl ImpersonationService {
    pub fn new(repo: &SqlxAuthRepo) -> Self {
        Self { repo: repo.clone() }
    }

    /// Start impersonating `target_user_id`. The `actor_id` is the real admin.
    /// Returns the target user's info for the session to store.
    pub async fn start(
        &self,
        actor_id: &str,
        target_user_id: &str,
    ) -> Result<String, ImpersonationError> {
        let target = self
            .repo
            .get_by_id(target_user_id)
            .await
            .map_err(|e| ImpersonationError::Internal(e.to_string()))?
            .ok_or(ImpersonationError::UserNotFound)?;

        if !target.active {
            return Err(ImpersonationError::TargetInactive);
        }

        let event = AuditEvent::new(AuditAction::ImpersonationStarted)
            .with_actor(actor_id)
            .with_impersonated_as(target_user_id)
            .with_entity("user", target_user_id);
        self.repo
            .log(&event)
            .await
            .map_err(|e| ImpersonationError::Internal(e.to_string()))?;

        Ok(target_user_id.to_string())
    }

    /// Stop impersonation. `impersonated_as` is the user that was being
    /// impersonated (from the session).
    pub async fn stop(
        &self,
        actor_id: &str,
        impersonated_as: &str,
    ) -> Result<(), ImpersonationError> {
        let event = AuditEvent::new(AuditAction::ImpersonationStopped)
            .with_actor(actor_id)
            .with_impersonated_as(impersonated_as)
            .with_entity("user", impersonated_as);
        self.repo
            .log(&event)
            .await
            .map_err(|e| ImpersonationError::Internal(e.to_string()))?;
        Ok(())
    }
}
