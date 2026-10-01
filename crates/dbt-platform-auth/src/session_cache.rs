use crate::credential::OAuthSession;
use serde::{Deserialize, Serialize};

/// On-disk cache persisted to `~/.dbt/oauth_sessions.json`.
///
/// Modeled as a `Vec` to support multi-account; a single active session is used for MVP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthSessionCache {
    pub version: u8,
    pub sessions: Vec<OAuthSession>,
}
