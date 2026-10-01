//! Per-run resolution cache shared across git host clients.

use super::{GitUrlParts, ResolveMethod};

#[derive(Default)]
pub struct ResolveCache {
    entries: scc::HashMap<String, (String, ResolveMethod)>,
}

impl ResolveCache {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Cache entries are scoped by any inline auth token carried in the URL.
/// URLs that differ only in their `https://TOKEN@host/...` prefix map to
/// distinct cache entries — different credentials may see different views
/// of a ref. Ambient tokens (env `GITHUB_TOKEN`) are
/// resolved separately by `auth.rs` and never reach `GitUrlParts.token`,
/// so they all share the tokenless "default" bucket.
fn cache_key(parts: &GitUrlParts, revision: &str) -> String {
    let token = parts.token.as_deref().unwrap_or("");
    format!(
        "{}/{}/{}/{}/{}",
        parts.host, parts.owner, parts.repo, token, revision
    )
}

impl ResolveCache {
    pub async fn get(
        &self,
        parts: &GitUrlParts,
        revision: &str,
    ) -> Option<(String, ResolveMethod)> {
        let key = cache_key(parts, revision);
        self.entries.get_async(&key).await.map(|e| e.get().clone())
    }

    pub async fn set(&self, parts: &GitUrlParts, revision: &str, sha: &str, method: ResolveMethod) {
        let key = cache_key(parts, revision);
        let _ = self
            .entries
            .insert_async(key, (sha.to_string(), method))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `GitUrlParts`. Tests use unique owner/repo pairs (via `tag`) so
    /// they never collide on the process-wide cache.
    fn parts(tag: &str, host: &str, token: Option<&str>) -> GitUrlParts {
        GitUrlParts {
            host: host.into(),
            owner: format!("owner-{tag}"),
            repo: format!("repo-{tag}"),
            token: token.map(str::to_string),
        }
    }

    #[dbt_runtime::test]
    async fn roundtrip_hit_and_miss() {
        let cache = ResolveCache::new();
        let p = parts("rt", "github.com", None);
        assert!(cache.get(&p, "v1").await.is_none(), "expected cold miss");

        cache.set(&p, "v1", "sha-v1", ResolveMethod::Identity).await;
        let (sha, method) = cache.get(&p, "v1").await.expect("expected hit");
        assert_eq!(sha, "sha-v1");
        assert!(matches!(method, ResolveMethod::Identity));

        // Different revision = different key = miss.
        assert!(cache.get(&p, "v2").await.is_none());
    }

    #[dbt_runtime::test]
    async fn token_scopes_cache_entries() {
        let cache = ResolveCache::new();
        let none = parts("tok", "github.com", None);
        let a = parts("tok", "github.com", Some("tokA"));
        let b = parts("tok", "github.com", Some("tokB"));

        cache
            .set(&none, "main", "sha-none", ResolveMethod::Identity)
            .await;
        cache
            .set(&a, "main", "sha-A", ResolveMethod::Identity)
            .await;
        cache
            .set(&b, "main", "sha-B", ResolveMethod::Identity)
            .await;

        assert_eq!(cache.get(&none, "main").await.unwrap().0, "sha-none");
        assert_eq!(cache.get(&a, "main").await.unwrap().0, "sha-A");
        assert_eq!(cache.get(&b, "main").await.unwrap().0, "sha-B");
    }

    #[dbt_runtime::test]
    async fn host_owner_repo_scoped_independently() {
        let cache = ResolveCache::new();
        let gh = parts("hsc", "github.com", None);
        let gl = parts("hsc", "gitlab.com", None);

        cache
            .set(&gh, "main", "sha-gh", ResolveMethod::GithubGraphql)
            .await;
        // Same owner/repo, different host → separate bucket.
        assert!(cache.get(&gl, "main").await.is_none());

        cache
            .set(&gl, "main", "sha-gl", ResolveMethod::GitLsRemote)
            .await;
        assert_eq!(cache.get(&gh, "main").await.unwrap().0, "sha-gh");
        assert_eq!(cache.get(&gl, "main").await.unwrap().0, "sha-gl");
    }

    #[dbt_runtime::test]
    async fn resolve_method_round_trips() {
        let cache = ResolveCache::new();
        // The cache records how a ref was resolved so subsequent hits can
        // attribute the method correctly to telemetry.
        let p_gql = parts("rm-gql", "github.com", None);
        cache
            .set(&p_gql, "v1", "sha-v1", ResolveMethod::GithubGraphql)
            .await;
        assert!(matches!(
            cache.get(&p_gql, "v1").await.unwrap().1,
            ResolveMethod::GithubGraphql
        ));

        let p_lsr = parts("rm-lsr", "github.com", None);
        cache
            .set(&p_lsr, "v1", "sha-v1", ResolveMethod::GitLsRemote)
            .await;
        assert!(matches!(
            cache.get(&p_lsr, "v1").await.unwrap().1,
            ResolveMethod::GitLsRemote
        ));
    }
}
