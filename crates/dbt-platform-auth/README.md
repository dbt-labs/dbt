# dbt-platform-auth

Credential types and resolver chain for authenticating requests to dbt platform.

## Overview

The crate provides:

- **`Credential`** — the runtime representation of a resolved credential (service token, PAT, or OAuth session)
- **`OAuthSession`** / **`OAuthSessionCache`** — types for the on-disk OAuth session cache
- **`AuthChain`** / **`AuthChainBuilder`** — an ordered chain of resolvers tried in sequence until one succeeds
- Individual **resolvers**: `EnvVarResolver`, `CloudYamlResolver`, `OAuthPassiveResolver`, `OAuthInteractiveResolver`

## Credential types

```rust
pub enum Credential {
    ServiceToken { token, account_host, account_id },
    Pat { token, account_host, account_id },
    OAuth(OAuthSession),
}
```

Tokens are classified automatically by prefix: `dbtu_` → `Pat`, anything else → `ServiceToken`.

## Usage

### Default chain (non-interactive)

The default chain tries resolvers in this order without any user interaction:

1. **`EnvVarResolver`** — `DBT_CLOUD_ACCOUNT_HOST`, `DBT_CLOUD_TOKEN`, `DBT_CLOUD_ACCOUNT_ID`
2. **`OAuthPassiveResolver`** — reads `~/.dbt/oauth_sessions.json`; returns a valid cached session, or attempts a refresh if the access token is expired and a refresh token is present *(token refresh not yet implemented)*
3. **`CloudYamlResolver`** — `./dbt_cloud.yml`, then `~/.dbt/dbt_cloud.yml`

```rust
use dbt_platform_auth::AuthChainBuilder;

let chain = AuthChainBuilder::default().build();
let credential = chain.resolve().await?;

println!("host:  {}", credential.account_host());
println!("token: {}", credential.token());
```

### Interactive chain

For contexts that can prompt the user (e.g. a `login` command), call `.interactive()` on the builder to append `OAuthInteractiveResolver` as a final fallback — a browser-based OAuth authorization code flow that runs only if all passive sources fail.

```rust
use dbt_platform_auth::AuthChainBuilder;

let chain = AuthChainBuilder::default().interactive().build();
let credential = chain.resolve().await?;
```

## OAuth resolvers

### `OAuthPassiveResolver`

Reads the OAuth session cache at `~/.dbt/oauth_sessions.json` (an `OAuthSessionCache`). Resolution logic:

1. Find the first session whose `expires_at` is still in the future → return it.
2. Find the first session that has a `refresh_token` → attempt to exchange it for a new access token *(not yet implemented; returns `AuthenticationExpired` as a placeholder)*.
3. No usable sessions → `NotAuthenticated` (empty cache) or `AuthenticationExpired` (all expired, no refresh token).

The cache path can be overridden for testing or non-standard installs:

```rust
use dbt_platform_auth::resolver::OAuthPassiveResolver;
use std::path::PathBuf;

let resolver = OAuthPassiveResolver {
    client_id: "my-client-id".into(),
    cache_path: Some(PathBuf::from("/custom/path/oauth_sessions.json")),
};
```

### `OAuthInteractiveResolver`

Opens a browser to the dbt platform authorization endpoint and captures the authorization code via a local redirect server. The resulting session is persisted to the OAuth session cache.

> **Not yet implemented.** Present in the interactive chain as a seam for the upcoming browser flow.

```rust
use dbt_platform_auth::resolver::OAuthInteractiveResolver;

let resolver = OAuthInteractiveResolver::new("my-client-id");
```

## Filtering the default chain

Use `AuthChainBuilder` to restrict which resolvers are active:

```rust
use dbt_platform_auth::{AuthChainBuilder, ResolverKind};

// Only look at environment variables — skip file and OAuth sources.
let chain = AuthChainBuilder::new()
    .allow_only(&[ResolverKind::EnvVar])
    .build();

// Use everything except the passive OAuth resolver.
let chain = AuthChainBuilder::new()
    .deny(&[ResolverKind::OAuthPassive])
    .build();
```

Available `ResolverKind` variants: `EnvVar`, `CloudYaml`, `OAuthPassive`, `OAuthInteractive`.

Note: `OAuthInteractive` is not included in the default chain — it only appears in the chain returned by `AuthChain::interactive()`.

## Custom resolver chain

Build an entirely custom chain with `with_resolvers`:

```rust
use dbt_platform_auth::{
    AuthChainBuilder,
    resolver::{AuthResolver, CloudYamlResolver},
};
use std::path::PathBuf;

// Resolve from a project-specific config file only.
let chain = AuthChainBuilder::with_resolvers(vec![
    AuthResolver::CloudYaml(CloudYamlResolver {
        path: Some(PathBuf::from("/path/to/project/dbt_cloud.yml")),
    }),
])
.build();

let credential = chain.resolve().await?;
```

## Error handling

`resolve()` returns `Result<Credential, AuthError>`:

| Variant | Meaning |
|---|---|
| `NotAuthenticated` | No resolver produced credentials |
| `AuthenticationExpired` | Credentials were found but have expired |
| `InaccessibleSource` | A source (file, socket) could not be read |
| `Malformed` | A source was readable but contained invalid data |

The chain continues past `InaccessibleSource` and `Malformed` errors rather than aborting. If credentials are never found, the first non-`NotAuthenticated` error is returned so the caller can surface the most actionable diagnosis.

```rust
use dbt_platform_auth::{AuthChainBuilder, AuthError};

match AuthChainBuilder::default().build().resolve().await {
    Ok(cred) => { /* use cred */ }
    Err(AuthError::NotAuthenticated) => eprintln!("no credentials found"),
    Err(AuthError::AuthenticationExpired) => eprintln!("credentials have expired — run `dbt login` to re-authenticate"),
    Err(AuthError::InaccessibleSource(e)) => eprintln!("could not read credential source: {e}"),
    Err(AuthError::Malformed(msg)) => eprintln!("credential source is invalid: {msg}"),
}
```
