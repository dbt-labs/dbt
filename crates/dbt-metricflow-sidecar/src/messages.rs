//! Wire message types for the mf-ipc v1 protocol.
//!
//! Mirrors `sidecar/mf_ipc_protocol.py` in the `metricflow` repo field-for-field.
//! That file is the source of truth for the wire shape — if the two ever
//! disagree, trust the Python side and update this file to match, not the
//! other way around.

use serde::{Deserialize, Serialize};

/// Startup handshake, written once by the sidecar before any request is read.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadyMessage {
    pub status: String,
    pub metricflow_version: String,
    pub python_version: String,
    pub protocol_version: u32,
}

/// Written instead of [`ReadyMessage`] if manifest pre-loading fails at startup.
#[derive(Debug, Clone, Deserialize)]
pub struct StartupErrorMessage {
    pub status: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

/// Outgoing request envelope. `method` is always a `'static` string literal
/// per call site (`"explain"`, `"ping"`, `"shutdown"`) — this protocol does not
/// use serde's tagged-enum convention, so a generic wrapper is simplest.
#[derive(Debug, Serialize)]
pub struct Request<P: Serialize> {
    pub id: String,
    pub method: &'static str,
    pub v: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<P>,
}

impl Request<()> {
    pub fn ping(id: String) -> Self {
        Self {
            id,
            method: "ping",
            v: 1,
            params: None,
        }
    }

    pub fn shutdown(id: String) -> Self {
        Self {
            id,
            method: "shutdown",
            v: 1,
            params: None,
        }
    }
}

impl Request<ExplainParams> {
    pub fn explain(id: String, params: ExplainParams) -> Self {
        Self {
            id,
            method: "explain",
            v: 1,
            params: Some(params),
        }
    }
}

/// Params for the `explain` method.
///
/// `sql_engine` is a plain string, not an enum: `mf_entry.py` looks the engine
/// up by enum *member name* (`SqlEngine["DUCKDB"]`), not by value. This must
/// stay a string to match the wire contract MetricFlow actually accepts.
#[derive(Debug, Clone, Serialize)]
pub struct ExplainParams {
    pub manifest_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_by_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub where_constraints: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_by_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    pub sql_engine: String,
}

/// The `error` payload of an error response.
#[derive(Debug, Clone, Deserialize)]
pub struct ErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
    #[serde(default)]
    pub traceback: Option<String>,
}

/// A response `id` as it actually appears on the wire. Requests we send
/// always carry a `String` id (see `Request`), but responses are parsed
/// defensively since the protocol technically allows `str | int | null`
/// (e.g. `id: null` for a request that couldn't be parsed at all).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ResponseId {
    Str(String),
    Int(i64),
}

/// Flat, permissive deserialization target for any response line. Kept
/// deliberately loose (every field but `id`/`ok` optional) rather than an
/// untagged enum over `ExplainResponse`/`OkResponse`/`ErrorResponse`: serde's
/// untagged matching tries variants in declaration order and accepts the
/// first one that parses, which is a real footgun here since a variant with
/// only `{id, ok}` would spuriously match a response that also has `sql` or
/// `error` fields sitting right next to it. Parsing into this struct first
/// and branching explicitly in `into_kind` makes the discrimination logic
/// visible instead of implicit in field-declaration order.
#[derive(Debug, Clone, Deserialize)]
struct RawResponse {
    id: Option<ResponseId>,
    ok: bool,
    #[serde(default)]
    sql: Option<String>,
    #[serde(default)]
    error: Option<ErrorDetail>,
}

/// A parsed, unambiguous response to a single request.
#[derive(Debug, Clone)]
pub enum ResponseKind {
    /// `ping` / `shutdown` success.
    Ok,
    /// `explain` success.
    Explain { sql: String },
    /// Any failed request, regardless of method.
    Error { error: ErrorDetail },
}

/// Parse one NDJSON response line into `(id, ResponseKind)`.
///
/// Returns `Err` if the line isn't valid JSON, doesn't match the expected
/// response shape, or its `id` isn't a string (which should never happen for
/// a response to a request *we* sent, since `Request::id` is always a
/// `String` — an int or null id here means the sidecar is responding to
/// something it couldn't associate with our request, e.g. a parse failure
/// on its end).
pub fn parse_response_line(line: &str) -> Result<(String, ResponseKind), String> {
    let raw: RawResponse =
        serde_json::from_str(line).map_err(|e| format!("invalid response JSON: {e}"))?;

    let id = match raw.id {
        Some(ResponseId::Str(s)) => s,
        other => {
            return Err(format!(
                "response id is not a string we could have sent: {other:?}"
            ));
        }
    };

    let kind = if raw.ok {
        match raw.sql {
            Some(sql) => ResponseKind::Explain { sql },
            None => ResponseKind::Ok,
        }
    } else {
        match raw.error {
            Some(error) => ResponseKind::Error { error },
            None => return Err("ok:false response missing an error payload".to_string()),
        }
    };

    Ok((id, kind))
}
