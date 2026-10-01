//! GitHub token resolution.
//!
//! We intentionally read from env only (`GITHUB_TOKEN`) and avoid shelling out
//! to git credential helpers in this layer.

/// Get a GitHub token from the environment, if present.
pub fn get_github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
}

/// Resolve auth token for a GitHub request.
///
/// Inline URL token has priority. If missing/empty, we fall back to
/// `GITHUB_TOKEN` from env.
pub fn github_token_for_request(inline_token: Option<&str>) -> Option<String> {
    inline_token
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .or_else(get_github_token)
}
