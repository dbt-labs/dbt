//! Minimal stand-in for `sidecar/mf_entry.py`, speaking just enough of
//! mf-ipc v1 to exercise `MetricflowSidecarManager`'s plumbing without any
//! dependency on Python or a real Nuitka binary.
//!
//! Behavior, by `method`:
//! - `ping` / `shutdown` -> `{"id","ok":true}` (`shutdown` also exits 0 after)
//! - `explain` with `params.manifest_path == "FORCE_ERROR"` -> a canned error response
//! - any other `explain` -> a canned `{"id","ok":true,"sql":"SELECT 1"}`
//! - unrecognised `method` -> a canned error response
//!
//! Deliberately parses incoming requests as bare `serde_json::Value` rather
//! than reusing this crate's own message types: those are one-directional
//! (`Request<P>` is `Serialize`-only, `ReadyMessage`/`ErrorDetail` are
//! `Deserialize`-only) since the real client only ever sends requests and
//! parses responses. This fixture plays the opposite role.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(
        out,
        "{}",
        json!({
            "status": "ready",
            "metricflow_version": "0.0.0-fake",
            "python_version": "0.0.0-fake",
            "protocol_version": 1,
        })
    )
    .expect("failed to write ready message");
    out.flush().expect("failed to flush ready message");

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.expect("failed to read stdin line");
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("fake_mf_entry: failed to parse request: {e}");
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");

        let response = match method {
            "ping" => json!({"id": id, "ok": true}),
            "shutdown" => json!({"id": id, "ok": true}),
            "explain" => {
                let manifest_path = request
                    .pointer("/params/manifest_path")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if manifest_path == "FORCE_ERROR" {
                    json!({
                        "id": id,
                        "ok": false,
                        "error": {"type": "TestError", "message": "forced error for testing"},
                    })
                } else {
                    json!({"id": id, "ok": true, "sql": "SELECT 1"})
                }
            }
            other => json!({
                "id": id,
                "ok": false,
                "error": {"type": "UnknownMethod", "message": format!("unknown method: {other}")},
            }),
        };

        writeln!(out, "{response}").expect("failed to write response");
        out.flush().expect("failed to flush response");

        if method == "shutdown" {
            break;
        }
    }
}
