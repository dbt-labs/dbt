# Snowflake `context deadline exceeded` repro

Reproduce gosnowflake's login error end-to-end (no real Snowflake needed):

```text
context deadline exceeded (Client.Timeout exceeded while awaiting headers)
```

## Why this error happens (Go)

In gosnowflake, each HTTP call uses `http.Client` with a timeout. When the request (connect, wait for headers, read body) exceeds that limit, Go's `context.Context` is cancelled and you get **`context deadline exceeded`**. It is a timeout signal: stop waiting and fail the request so the client does not hang forever.

Here the black-hole server never sends headers, so the client waits until `AUTH_CLIENT_TIMEOUT` fires.

## Run

**Terminal 1** — black-hole on `127.0.0.1:9999` (must match `profiles.yml`):

```sh
python3 blackhole.py
```

**Terminal 2** — set `DBT_SNOWFLAKE_AUTH_CLIENT_TIMEOUT` so the repro fails fast (default is 900s):

```sh
export DBT_SNOWFLAKE_AUTH_CLIENT_TIMEOUT=1ms
fsd run --profiles-dir . --target snowflake
```

Expect a connection error containing `Client.Timeout exceeded while awaiting headers`.

## Rust unit test

`../snowflake_context_deadline_exceeded.rs` — same setup in-process; sets `AUTH_CLIENT_TIMEOUT=1ms` on the builder instead of the env var.

## Fusion vs Python dbt — comparison summary

**Setup:** `host: 127.0.0.1`, `port: 9999` — TCP accepts but never sends HTTP headers (simulates hung login).

### Results

| | Python dbt-snowflake | Fusion (fsd) |
|---|---|---|
| Total time | ~2 min (~121s) | ~40–44 min |
| Connection opens | 1 | 2 (`list_schemas` → `create_schema`) |
| Login attempts per open | 2 auth attempts (connector-level) | 2 outer (fs `connect_retries=1`) × ~8 inner HTTP retries (gosnowflake) |
| Final error | `Could not connect after 2 attempt(s)` | `Failed to create schema 'stub-schema'...` |

### Why Python is ~2 min

- dbt-snowflake calls `connect()` once — login `OperationalError` is **not** in dbt's retryable list
- snowflake-connector-python does 2 auth attempts (`DEFAULT_MAX_CON_RETRY_ATTEMPTS=1`)
- Each attempt hits 60s socket read timeout → **2 × 60s ≈ 120s**

### Why Fusion is ~40–44 min

- Pre-run schema registration does two warehouse calls, each needing a fresh connection (login never succeeds, so thread-local reuse doesn't kick in):
  1. `list_schemas` — check if schema exists
  2. `create_schema` — create missing schema (still attempted after #1 fails)
- Each `new_connection_with_config` has fs `connect_retries=1` → **2 outer attempts**
- Each attempt lets gosnowflake run up to **8 HTTP retries** at **60s** each (~10 min per open)
- **~10 min × 2 retries × 2 opens ≈ 40–44 min**

### Takeaway

Python fails fast on connect and stops. Fusion retries login errors aggressively at both the fs and gosnowflake layers, then still proceeds to schema creation after the first open fails — compounding the wait.
