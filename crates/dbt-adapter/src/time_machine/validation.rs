//! SQL diffing utilities for evaluating replay of recorded events.
//!
//! This module provides a validation engine for comparing incoming events against
//! recorded events, with support for:
//! - Known deviations that should skip validation
//! - SQL extraction and sanitization for meaningful comparison
//! - Pluggable sanitizers to clean dynamic content from SQL
//!   (including stripping `--` / `/* */` comments so diagnostics do not false-fail replay)

use std::{fmt, sync::LazyLock};

use dbt_sql_utils::sql_split_statements;
use regex::Regex;
use similar::{ChangeTag, TextDiff};

use crate::AdapterType;
use crate::sql::diff::PrivilegeStatement;
use crate::sql::normalize::strip_sql_comments;
use crate::time_machine::event::AdapterCallEvent;

/// An incoming event being compared against a recorded event.
#[derive(Debug, Clone)]
pub struct IncomingEvent<'a> {
    pub node_id: &'a str,
    pub method: &'a str,
    pub args: &'a serde_json::Value,
}

impl<'a> IncomingEvent<'a> {
    pub fn new(node_id: &'a str, method: &'a str, args: &'a serde_json::Value) -> Self {
        Self {
            node_id,
            method,
            args,
        }
    }
}

impl<'a> From<(&'a str, &'a str, &'a serde_json::Value)> for IncomingEvent<'a> {
    fn from((node_id, method, args): (&'a str, &'a str, &'a serde_json::Value)) -> Self {
        Self::new(node_id, method, args)
    }
}

/// An event with SQL extracted and sanitized for comparison.
#[derive(Debug, Clone)]
pub struct SqlEvent<'a> {
    /// The original node ID
    pub _node_id: &'a str,
    /// The method name
    pub _method: &'a str,
    /// The sanitized SQL string
    pub sanitized_sql: String,
    /// The original raw SQL before sanitization
    pub raw_sql: &'a str,
    /// Non-SQL args
    pub _other_args: serde_json::Value,
}

impl<'a> SqlEvent<'a> {
    /// Create from an incoming event, extracting and sanitizing SQL.
    pub fn from_incoming(
        incoming: &'a IncomingEvent<'a>,
        sanitizers: &[Box<dyn SqlSanitizer>],
    ) -> Option<Self> {
        let raw_sql = extract_sql_from_args(incoming.args)?;
        let sanitized_sql = apply_sanitizers(raw_sql, sanitizers);
        let other_args = extract_non_sql_args(incoming.args);

        Some(Self {
            _node_id: incoming.node_id,
            _method: incoming.method,
            sanitized_sql,
            raw_sql,
            _other_args: other_args,
        })
    }

    /// Create from a recorded event, extracting and sanitizing SQL.
    pub fn from_recorded(
        recorded: &'a AdapterCallEvent,
        sanitizers: &[Box<dyn SqlSanitizer>],
    ) -> Option<Self> {
        let raw_sql = extract_sql_from_args(&recorded.args)?;
        let sanitized_sql = apply_sanitizers(raw_sql, sanitizers);
        let other_args = extract_non_sql_args(&recorded.args);

        Some(Self {
            _node_id: &recorded.node_id,
            _method: &recorded.method,
            sanitized_sql,
            raw_sql,
            _other_args: other_args,
        })
    }
}

// TODO(jason): Hook into telemetry or logging for skips or sanitization so we know what was applied
// would help debug artifacts that should be failing but are passing

/// Trait for known deviations that should skip validation.
///
/// Implement this trait to define patterns that are expected to differ
/// between recording and replay.
pub trait KnownDeviation: Send + Sync {
    /// Name of this deviation rule.
    fn name(&self) -> &str;

    /// Human-readable reason why this is an expected deviation.
    fn reason(&self) -> &str;

    /// Check if the incoming event matches this deviation pattern.
    fn check(&self, incoming: &IncomingEvent, recorded: &AdapterCallEvent) -> DeviationMatch<'_>;
}

/// Result of checking for a known deviation.
#[derive(Debug)]
pub enum DeviationMatch<'a> {
    /// No deviation rule matched - validation should proceed normally
    None,
    /// A known deviation was matched - validation should be skipped
    Matched(MatchedDeviation<'a>),
}

/// Details about a matched deviation.
#[derive(Debug)]
pub struct MatchedDeviation<'a> {
    /// Name of the deviation rule that matched
    #[allow(dead_code)]
    pub rule_name: &'a str,
    /// Human-readable reason why this is an expected deviation
    #[allow(dead_code)]
    pub reason: &'a str,
}

/// Trait for SQL sanitizers that clean dynamic content.
///
/// Implement this trait to define patterns in SQL that should be
/// normalized before comparison (e.g., timestamps, UUIDs, etc.)
pub trait SqlSanitizer: Send + Sync {
    /// Name of this sanitizer.
    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    /// Sanitize the SQL string, returning the cleaned version.
    fn sanitize(&self, sql: &str) -> String;
}

/// Result of event validation.
#[derive(Debug)]
pub enum ValidationResult<'a> {
    /// Events match
    Match,
    /// Validation was skipped due to a known deviation
    #[allow(dead_code)]
    Skipped(MatchedDeviation<'a>),
    /// Events differ
    Mismatch(ValidationMismatch),
}

/// Details about a validation mismatch.
#[derive(Debug)]
pub struct ValidationMismatch {
    /// Type of mismatch
    pub kind: MismatchKind,
    /// Expected value
    pub expected: String,
    /// Actual value
    pub actual: String,
}

/// Kind of mismatch found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MismatchKind {
    /// Method names don't match
    Method,
    /// SQL content differs
    Sql,
    /// Recorded event has SQL but incoming does not
    MissingSqlIncoming,
    /// Incoming event has SQL but recorded does not
    MissingSqlRecorded,
    /// Non-SQL arguments differ
    Args,
}

impl fmt::Display for ValidationMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            MismatchKind::Method => {
                write!(
                    f,
                    "method mismatch: expected '{}', got '{}'",
                    self.expected, self.actual
                )
            }
            MismatchKind::Sql => {
                writeln!(f, "SQL mismatch:")?;
                let diff = TextDiff::from_lines(&self.expected, &self.actual);
                for change in diff.iter_all_changes() {
                    let prefix = match change.tag() {
                        ChangeTag::Delete => "-",
                        ChangeTag::Insert => "+",
                        ChangeTag::Equal => " ",
                    };
                    write!(f, "{}{}", prefix, change)?;
                }
                Ok(())
            }
            MismatchKind::MissingSqlIncoming => {
                write!(
                    f,
                    "SQL missing in incoming event, expected:\n{}",
                    self.expected
                )
            }
            MismatchKind::MissingSqlRecorded => {
                write!(f, "SQL missing in recorded event, got:\n{}", self.actual)
            }
            MismatchKind::Args => {
                write!(
                    f,
                    "args mismatch:\n  expected: {}\n  actual: {}",
                    self.expected, self.actual
                )
            }
        }
    }
}

/// The main validation engine for time machine event comparison.
pub struct TimeMachineEventValidationEngine {
    /// Registered deviation rules
    deviations: Vec<Box<dyn KnownDeviation>>,
    /// Registered SQL sanitizers
    sanitizers: Vec<Box<dyn SqlSanitizer>>,
    /// Adapter type for non-SQL argument comparison
    adapter_type: AdapterType,
}

impl Default for TimeMachineEventValidationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeMachineEventValidationEngine {
    /// Create a new validation engine with default rules.
    ///
    /// Defaults to `AdapterType::Snowflake`; call `with_adapter_type` to override.
    pub fn new() -> Self {
        let mut engine = Self {
            deviations: Vec::new(),
            sanitizers: Vec::new(),
            adapter_type: AdapterType::Snowflake,
        };

        // Register deviations
        engine.register_deviation(Box::new(DbtPovModelCostCalculatorDeviation));
        engine.register_deviation(Box::new(DbtArtifactsDeviation));
        engine.register_deviation(Box::new(ElementaryDeviation));

        // Register sanitizers
        engine.register_sanitizer(Box::new(TimestampSanitizer));
        engine.register_sanitizer(Box::new(DateLiteralSanitizer));
        engine.register_sanitizer(Box::new(QueryTagSanitizer));
        engine.register_sanitizer(Box::new(UuidSanitizer));
        engine.register_sanitizer(Box::new(TmpSuffixSanitizer));
        engine.register_sanitizer(Box::new(GrantOrderingSanitizer));
        // Strip SQL comments before whitespace normalization so dynamic text in comments
        // (timestamps, state fingerprints, etc.) does not cause false mismatches.
        engine.register_sanitizer(Box::new(SqlCommentSanitizer));
        // Whitespace sanitizer should be last to normalize after other sanitizers
        engine.register_sanitizer(Box::new(WhitespaceSanitizer));

        engine
    }

    /// Set the adapter type used for non-SQL argument comparison.
    pub fn with_adapter_type(mut self, adapter_type: AdapterType) -> Self {
        self.adapter_type = adapter_type;
        self
    }

    /// Register a deviation rule.
    pub fn register_deviation(&mut self, deviation: Box<dyn KnownDeviation>) {
        self.deviations.push(deviation);
    }

    /// Register a SQL sanitizer.
    pub fn register_sanitizer(&mut self, sanitizer: Box<dyn SqlSanitizer>) {
        self.sanitizers.push(sanitizer);
    }

    /// Check if a node matches any registered deviation.
    pub fn is_known_nondeterministic_node(&self, node_id: &str) -> bool {
        let dummy_incoming = IncomingEvent::new(node_id, "", &serde_json::Value::Null);
        let dummy_recorded = AdapterCallEvent {
            node_id: node_id.to_string(),
            method: String::new(),
            semantic_category: super::SemanticCategory::Pure,
            args: serde_json::Value::Null,
            result: serde_json::Value::Null,
            success: true,
            error: None,
            seq: 0,
            timestamp_ns: 0,
        };
        self.deviations.iter().any(|d| {
            matches!(
                d.check(&dummy_incoming, &dummy_recorded),
                DeviationMatch::Matched(_)
            )
        })
    }

    /// Validate an incoming event against a recorded event.
    pub fn validate(
        &self,
        incoming: &IncomingEvent,
        recorded: &AdapterCallEvent,
    ) -> ValidationResult<'_> {
        for deviation in &self.deviations {
            if let DeviationMatch::Matched(matched) = deviation.check(incoming, recorded) {
                return ValidationResult::Skipped(matched);
            }
        }

        if incoming.method != recorded.method {
            return ValidationResult::Mismatch(ValidationMismatch {
                kind: MismatchKind::Method,
                expected: recorded.method.clone(),
                actual: incoming.method.to_string(),
            });
        }

        if is_sql_method(incoming.method) {
            return self.validate_sql_event(incoming, recorded);
        }

        // 4. For non SQL methods, compare args directly
        if !super::event_replay::adapter_args_match_for_type(
            incoming.method,
            &recorded.args,
            incoming.args,
            self.adapter_type,
        ) {
            return ValidationResult::Mismatch(ValidationMismatch {
                kind: MismatchKind::Args,
                expected: recorded.args.to_string(),
                actual: incoming.args.to_string(),
            });
        }

        ValidationResult::Match
    }

    /// Validate SQL events specifically.
    fn validate_sql_event(
        &self,
        incoming: &IncomingEvent,
        recorded: &AdapterCallEvent,
    ) -> ValidationResult<'_> {
        // Extract SQL from both events
        let incoming_sql = SqlEvent::from_incoming(incoming, &self.sanitizers);
        let recorded_sql = SqlEvent::from_recorded(recorded, &self.sanitizers);

        match (incoming_sql, recorded_sql) {
            (Some(inc), Some(rec)) => {
                // Compare using fully sanitized SQL
                if inc.sanitized_sql == rec.sanitized_sql {
                    ValidationResult::Match
                } else {
                    // Format the sanitized SQL for readable diff display
                    ValidationResult::Mismatch(ValidationMismatch {
                        kind: MismatchKind::Sql,
                        expected: format_sql_for_display(&rec.sanitized_sql),
                        actual: format_sql_for_display(&inc.sanitized_sql),
                    })
                }
            }
            (None, None) => ValidationResult::Match,
            (None, Some(inc)) => {
                // Incoming has SQL but recorded does not
                ValidationResult::Mismatch(ValidationMismatch {
                    kind: MismatchKind::MissingSqlRecorded,
                    expected: "(no SQL in recorded event)".to_string(),
                    actual: inc.raw_sql.to_string(),
                })
            }
            (Some(rec), None) => {
                // Recorded has SQL but incoming does not
                ValidationResult::Mismatch(ValidationMismatch {
                    kind: MismatchKind::MissingSqlIncoming,
                    expected: rec.raw_sql.to_string(),
                    actual: "(no SQL in incoming event)".to_string(),
                })
            }
        }
    }
}

fn is_sql_method(method: &str) -> bool {
    method == "execute" || method == "run_query" || method == "add_query"
}

/// Extract SQL string from args.
fn extract_sql_from_args(args: &serde_json::Value) -> Option<&str> {
    match args {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Array(arr) => arr.first().and_then(|v| v.as_str()),
        _ => None,
    }
}

/// Extract non SQL args
fn extract_non_sql_args(args: &serde_json::Value) -> serde_json::Value {
    match args {
        serde_json::Value::Array(arr) if !arr.is_empty() => {
            serde_json::Value::Array(arr[1..].to_vec())
        }
        _ => serde_json::Value::Array(vec![]),
    }
}

/// Apply all sanitizers to a SQL string.
fn apply_sanitizers(sql: &str, sanitizers: &[Box<dyn SqlSanitizer>]) -> String {
    let mut result = sql.to_string();
    for sanitizer in sanitizers {
        result = sanitizer.sanitize(&result);
    }
    result
}

// TODO(jason): A real SQL formatter...
/// Format normalized SQL for readable diff display by looking at certain keywords
fn format_sql_for_display(normalized_sql: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(SELECT|FROM|WHERE|AND|OR|JOIN|LEFT JOIN|RIGHT JOIN|INNER JOIN|OUTER JOIN|CROSS JOIN|ON|GROUP BY|ORDER BY|HAVING|LIMIT|OFFSET|UNION|INTERSECT|EXCEPT|WITH|AS \(|INSERT|UPDATE|DELETE|CREATE|ALTER|DROP|COPY GRANTS|VALUES)\b"
        ).expect("valid regex")
    });

    RE.replace_all(normalized_sql, "\n$1").trim().to_string()
}

// ============================================================================
// Deviations
// ============================================================================

/// Deviation for dbt_pov_model_cost_calculator package.
///
/// This package generates dynamic SQL with:
/// - Execution times
/// - Timestamps
/// - Invocation IDs
/// - Run IDs
pub struct DbtPovModelCostCalculatorDeviation;

impl KnownDeviation for DbtPovModelCostCalculatorDeviation {
    fn name(&self) -> &'static str {
        "dbt_pov_model_cost_calculator"
    }

    fn reason(&self) -> &'static str {
        "Package generates dynamic SQL with runtime metrics (execution times, timestamps, invocation IDs) that differ between runs"
    }

    fn check(&self, incoming: &IncomingEvent, _recorded: &AdapterCallEvent) -> DeviationMatch<'_> {
        if incoming.node_id.contains("dbt_pov_model_cost_calculator") {
            DeviationMatch::Matched(MatchedDeviation {
                rule_name: self.name(),
                reason: self.reason(),
            })
        } else {
            DeviationMatch::None
        }
    }
}

/// Deviation for the `dbt_artifacts` package.
///
/// dbt Artifacts hooks generate SQL with runtime-dependent data: timestamps, thread IDs,
/// node runtimes, and rows affected. These values are inherently non-deterministic across
/// runs and cannot be meaningfully compared.
pub struct DbtArtifactsDeviation;

impl KnownDeviation for DbtArtifactsDeviation {
    fn name(&self) -> &'static str {
        "dbt_artifacts"
    }

    fn reason(&self) -> &'static str {
        "dbt Artifacts embeds runtime data (timestamps, thread ids, node runtimes, rows affected) that differ between recording and replay"
    }

    fn check(&self, incoming: &IncomingEvent, _recorded: &AdapterCallEvent) -> DeviationMatch<'_> {
        // Direct match: node belongs to the dbt_artifacts package.
        if incoming.node_id.contains(".dbt_artifacts.") {
            return DeviationMatch::Matched(MatchedDeviation {
                rule_name: self.name(),
                reason: self.reason(),
            });
        }

        // Indirect match: dbt_artifacts registers on-run-end hooks under the consuming
        // project's namespace (`operation.<project>.<project>-on-run-end-N`). Detect
        // these by confirming the SQL is an INSERT INTO targeting a dbt_artifacts schema.
        // PR #10448 switched hook node IDs from underscores (`-on_run_end-`) to hyphens
        // (`-on-run-end-`); accept both so older recordings keep matching.
        if incoming.node_id.contains("-on-run-end-") || incoming.node_id.contains("-on_run_end-") {
            static RE: LazyLock<Regex> = LazyLock::new(|| {
                Regex::new(r"(?i)insert\s+into\s+[^\s(]*dbt_artifacts").expect("valid regex")
            });
            if extract_sql_from_args(incoming.args)
                .map(|sql| RE.is_match(sql))
                .unwrap_or(false)
            {
                return DeviationMatch::Matched(MatchedDeviation {
                    rule_name: self.name(),
                    reason: self.reason(),
                });
            }
        }

        DeviationMatch::None
    }
}

/// Deviation for the `elementary` observability package.
///
/// Elementary hooks generate SQL with runtime-dependent data: invocation IDs,
/// timestamps, run metadata, and test results. These values are inherently
/// non-deterministic across runs and cannot be meaningfully compared.
pub struct ElementaryDeviation;

impl KnownDeviation for ElementaryDeviation {
    fn name(&self) -> &'static str {
        "elementary"
    }

    fn reason(&self) -> &'static str {
        "Elementary package embeds runtime data (timestamps, invocation IDs, test results) that differ between recording and replay"
    }

    fn check(&self, incoming: &IncomingEvent, _recorded: &AdapterCallEvent) -> DeviationMatch<'_> {
        if incoming.node_id.contains(".elementary.") {
            DeviationMatch::Matched(MatchedDeviation {
                rule_name: self.name(),
                reason: self.reason(),
            })
        } else {
            DeviationMatch::None
        }
    }
}

/// Sanitizer for timestamp patterns (single or double-quoted).
///
/// Handles both ISO 8601 (`2026-02-13T14:49:54`) and space-separated
/// (`2026-02-13 14:49:54`) formats, with optional fractional seconds
/// and timezone offsets.
pub struct TimestampSanitizer;

impl SqlSanitizer for TimestampSanitizer {
    fn name(&self) -> &'static str {
        "timestamp"
    }

    fn sanitize(&self, sql: &str) -> String {
        // Matches both 'T' and space separators between date and time
        static SINGLE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"'\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[+-]\d{2}:\d{2})?'")
                .expect("valid regex")
        });
        static DOUBLE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#""\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[+-]\d{2}:\d{2})?""#)
                .expect("valid regex")
        });
        let sql = SINGLE.replace_all(sql, "'<TIMESTAMP>'");
        DOUBLE.replace_all(&sql, "\"<TIMESTAMP>\"").to_string()
    }
}

/// Sanitizer for quoted date literal patterns.
///
/// Replaces date literals like `'2026-02-12'` or `"2026-02-12"` with a
/// placeholder. This handles SQL that embeds `current_date()` or similar
/// expressions that produce different dates between recording and replay.
/// Double-quoted dates appear in Databricks SQL (e.g. `WHERE partition_date = "2026-02-12"`).
///
/// Must run after `TimestampSanitizer` so full timestamps are already replaced.
pub struct DateLiteralSanitizer;

impl SqlSanitizer for DateLiteralSanitizer {
    fn name(&self) -> &'static str {
        "date_literal"
    }

    fn sanitize(&self, sql: &str) -> String {
        static SINGLE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"'\d{4}-\d{2}-\d{2}'").expect("valid regex"));
        static DOUBLE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#""\d{4}-\d{2}-\d{2}""#).expect("valid regex"));
        let sql = SINGLE.replace_all(sql, "'<DATE>'");
        DOUBLE.replace_all(&sql, "\"<DATE>\"").to_string()
    }
}

/// Sanitizer for query_tag session settings.
///
/// Query tags contain dynamic values like thread IDs that differ between runs.
/// Example: `alter session set query_tag = '{"app": "dbt", "thread_id": "Thread-2"}'`
pub struct QueryTagSanitizer;

impl SqlSanitizer for QueryTagSanitizer {
    fn name(&self) -> &'static str {
        "query_tag"
    }

    fn sanitize(&self, sql: &str) -> String {
        // Match: alter session set query_tag = '...' (case insensitive)
        static RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)(alter\s+session\s+set\s+query_tag\s*=\s*)'[^']*'")
                .expect("valid regex")
        });
        RE.replace_all(sql, "${1}''").to_string()
    }
}

/// Sanitizer for quoted UUID string literals (single or double-quoted).
///
/// Replaces UUIDs like `'8f439b7e-752f-460a-8d1a-f469231d169c'` with `'<UUID>'`
/// and `"8f439b7e-752f-460a-8d1a-f469231d169c"` with `"<UUID>"`.
/// Double-quoted UUIDs appear inside JSON embedded in SQL comments
/// (e.g. elementary package metadata).
pub struct UuidSanitizer;

impl SqlSanitizer for UuidSanitizer {
    fn name(&self) -> &'static str {
        "uuid"
    }

    fn sanitize(&self, sql: &str) -> String {
        static SINGLE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'")
                .expect("valid regex")
        });
        static DOUBLE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"(?i)"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}""#)
                .expect("valid regex")
        });
        let sql = SINGLE.replace_all(sql, "'<UUID>'");
        DOUBLE.replace_all(&sql, "\"<UUID>\"").to_string()
    }
}

/// Sanitizer for `__dbt_tmp` relation-name suffixes.
///
/// Replaces `__dbt_tmp<digits>` (and bare `__dbt_tmp`) with `__dbt_tmpSUFFIX`.
/// Mirrors `canonicalize_dbt_model_tmp_suffix` in `sql::diff`.
pub struct TmpSuffixSanitizer;

impl SqlSanitizer for TmpSuffixSanitizer {
    fn name(&self) -> &'static str {
        "tmp_suffix"
    }

    fn sanitize(&self, sql: &str) -> String {
        if !sql.to_ascii_lowercase().contains("__dbt_tmp") {
            return sql.to_string();
        }
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?i)(__dbt_tmp)_?\d*\b").expect("valid regex"));
        RE.replace_all(sql, "${1}SUFFIX").to_string()
    }
}

/// Sanitizer that normalizes the ordering of semicolon-separated GRANT/REVOKE privilege
/// statements within contiguous verb groups.
///
/// Some hooks generate a batch of privilege statements whose iteration order is
/// non-deterministic. When the entire SQL is a same-target sequence of simple
/// GRANT/REVOKE statements, this sanitizer sorts statements inside contiguous
/// GRANT or REVOKE runs while preserving the run order itself.
pub struct GrantOrderingSanitizer;

impl SqlSanitizer for GrantOrderingSanitizer {
    fn name(&self) -> &'static str {
        "grant_ordering"
    }

    fn sanitize(&self, sql: &str) -> String {
        let statements = sql_split_statements(sql.trim(), None);
        if statements.len() < 2 {
            return sql.to_string();
        }

        static PRIVILEGE_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r#"(?is)^\s*(?P<verb>grant|revoke)\s+(?P<privilege>.+?)\s+on\s+(?P<object>.+?)\s+(?P<direction>to|from)\s+(?P<principal>.+?)\s*$"#,
            )
            .expect("valid privilege regex")
        });

        let mut parsed = Vec::with_capacity(statements.len());
        for stmt in statements {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }

            let Some(caps) = PRIVILEGE_RE.captures(stmt) else {
                return sql.to_string();
            };

            let verb = caps["verb"].trim().to_ascii_lowercase();
            let direction = caps["direction"].trim().to_ascii_lowercase();
            if (verb == "grant" && direction != "to") || (verb == "revoke" && direction != "from") {
                return sql.to_string();
            }

            parsed.push(PrivilegeStatement {
                verb,
                privilege: caps["privilege"].trim().to_string(),
                object: caps["object"].trim().to_string(),
                principal: caps["principal"].trim().to_string(),
            });
        }

        let Some(first) = parsed.first() else {
            return sql.to_string();
        };
        if parsed
            .iter()
            .any(|stmt| stmt.object != first.object || stmt.principal != first.principal)
        {
            return sql.to_string();
        }

        let mut out = Vec::with_capacity(parsed.len());
        let mut start = 0usize;
        while start < parsed.len() {
            let verb = parsed[start].verb.clone();
            let mut end = start + 1;
            while end < parsed.len() && parsed[end].verb == verb {
                end += 1;
            }

            let mut group = parsed[start..end].to_vec();
            group.sort_by(|a, b| {
                a.privilege
                    .cmp(&b.privilege)
                    .then_with(|| a.render().cmp(&b.render()))
            });
            out.extend(group.into_iter().map(|stmt| stmt.render()));
            start = end;
        }

        out.join(" ")
    }
}

/// Sanitizer that removes SQL line and block comments using the same string-aware
/// implementation as query-cache key normalization ([`crate::sql::normalize::strip_sql_comments`]).
///
/// Executed SQL often includes non-semantic `--` / `/* */` diagnostics (e.g. frozen state
/// timestamps) that differ between recording and replay without changing executable SQL.
pub struct SqlCommentSanitizer;

impl SqlSanitizer for SqlCommentSanitizer {
    fn name(&self) -> &'static str {
        "sql_comments"
    }

    fn sanitize(&self, sql: &str) -> String {
        strip_sql_comments(sql)
    }
}

/// Sanitizer that normalizes whitespace by collapsing consecutive whitespace
/// characters into single spaces.
///
/// This handles cases where SQL semantically equivalent but differs only in
/// whitespace formatting (e.g., extra newlines between keywords).
pub struct WhitespaceSanitizer;

impl SqlSanitizer for WhitespaceSanitizer {
    fn name(&self) -> &'static str {
        "whitespace"
    }

    fn sanitize(&self, sql: &str) -> String {
        // Collapse all consecutive whitespace (spaces, tabs, newlines) into single space
        static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid regex"));
        RE.replace_all(sql, " ").trim().to_string()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_machine::semantic::SemanticCategory;

    fn make_recorded_event(
        node_id: &str,
        method: &str,
        args: serde_json::Value,
    ) -> AdapterCallEvent {
        AdapterCallEvent {
            node_id: node_id.to_string(),
            seq: 0,
            method: method.to_string(),
            semantic_category: SemanticCategory::Write,
            args,
            result: serde_json::json!(null),
            success: true,
            error: None,
            timestamp_ns: 0,
        }
    }

    #[test]
    fn test_dbt_pov_model_cost_calculator_deviation() {
        let engine = TimeMachineEventValidationEngine::new();
        let args = serde_json::json!([]);

        let incoming = IncomingEvent::new(
            "operation.dbt_pov_model_cost_calculator.dbt_pov_model_cost_calculator-on_run_end-0",
            "execute",
            &args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", args.clone());

        let result = engine.validate(&incoming, &recorded);
        assert!(matches!(result, ValidationResult::Skipped(_)));
    }

    #[test]
    fn test_normal_model_validates() {
        let engine = TimeMachineEventValidationEngine::new();
        let args = serde_json::json!(["SELECT 1"]);

        let incoming = IncomingEvent::new("model.my_project.my_model", "execute", &args);
        let recorded = make_recorded_event(incoming.node_id, "execute", args.clone());

        let result = engine.validate(&incoming, &recorded);
        assert!(matches!(result, ValidationResult::Match));
    }

    #[test]
    fn test_sql_mismatch_detected() {
        let engine = TimeMachineEventValidationEngine::new();
        let incoming_args = serde_json::json!(["SELECT 1"]);
        let recorded_args = serde_json::json!(["SELECT 2"]);

        let incoming = IncomingEvent::new("model.my_project.my_model", "execute", &incoming_args);
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        let result = engine.validate(&incoming, &recorded);
        assert!(matches!(
            result,
            ValidationResult::Mismatch(ValidationMismatch {
                kind: MismatchKind::Sql,
                ..
            })
        ));
    }

    #[test]
    fn test_timestamp_sanitizer() {
        let sanitizer = TimestampSanitizer;
        let sql = "INSERT INTO t (ts) VALUES ('2026-01-21T01:04:12.066273')";
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(sanitized, "INSERT INTO t (ts) VALUES ('<TIMESTAMP>')");
    }

    #[test]
    fn test_date_literal_sanitizer() {
        let sanitizer = DateLiteralSanitizer;
        let sql = "WHERE partition_date = '2026-02-12' AND last_seen_date < '2026-02-12'";
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(
            sanitized,
            "WHERE partition_date = '<DATE>' AND last_seen_date < '<DATE>'"
        );
    }

    #[test]
    fn test_date_literal_sanitizer_double_quoted() {
        let sanitizer = DateLiteralSanitizer;
        let sql = r#"OPTIMIZE db.schema.tbl WHERE partition_date = "2026-02-11" ZORDER BY (puuid)"#;
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(
            sanitized,
            r#"OPTIMIZE db.schema.tbl WHERE partition_date = "<DATE>" ZORDER BY (puuid)"#
        );
    }

    #[test]
    fn test_date_literal_sanitizer_does_not_match_timestamps() {
        // TimestampSanitizer runs first; after that, only bare dates remain.
        // But standalone, DateLiteralSanitizer should not corrupt timestamps
        // because dates inside timestamps don't appear as 'YYYY-MM-DD' alone.
        let sanitizer = DateLiteralSanitizer;
        let sql = "VALUES ('2026-01-23T00:49:47.760170')";
        let sanitized = sanitizer.sanitize(sql);
        // The pattern 'YYYY-MM-DD' doesn't match inside a timestamp
        // because the timestamp continues past the date portion
        assert_eq!(sanitized, "VALUES ('2026-01-23T00:49:47.760170')");
    }

    #[test]
    fn test_tmp_suffix_sanitizer_normalizes_bare_and_suffixed() {
        let sanitizer = TmpSuffixSanitizer;
        let bare = "create or replace table `p`.`d`.`snap__dbt_tmp` as ( select 1 )";
        let suffixed =
            "create or replace table `p`.`d`.`snap__dbt_tmp152555838935494` as ( select 1 )";
        let with_underscore =
            "create or replace table `p`.`d`.`snap__dbt_tmp_152555838935494` as ( select 1 )";
        assert_eq!(sanitizer.sanitize(bare), sanitizer.sanitize(suffixed));
        assert_eq!(
            sanitizer.sanitize(bare),
            sanitizer.sanitize(with_underscore)
        );
        assert_eq!(
            sanitizer.sanitize(bare),
            "create or replace table `p`.`d`.`snap__dbt_tmpSUFFIX` as ( select 1 )"
        );
    }

    #[test]
    fn test_tmp_suffix_sanitizer_makes_replay_match() {
        let engine = TimeMachineEventValidationEngine::new();
        let recorded_sql = "create or replace table `p`.`d`.`snap__dbt_tmp` OPTIONS( expiration_timestamp=TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL 12 hour) ) as ( select 1 )";
        let incoming_sql = "create or replace table `p`.`d`.`snap__dbt_tmp152555838935494` OPTIONS( expiration_timestamp=TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL 12 hour) ) as ( select 1 )";

        let recorded_args = serde_json::json!([recorded_sql]);
        let incoming_args = serde_json::json!([incoming_sql]);
        let incoming = IncomingEvent::new(
            "snapshot.motherbrain_data.snapshot_salesforce_lead_id_changes",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        let result = engine.validate(&incoming, &recorded);
        assert!(matches!(result, ValidationResult::Match));
    }

    #[test]
    fn test_sanitization_makes_date_sql_match() {
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args = serde_json::json!([
            "SELECT * FROM t WHERE partition_date = '2026-02-16' AND last_seen_date < '2026-02-16'"
        ]);
        let recorded_args = serde_json::json!([
            "SELECT * FROM t WHERE partition_date = '2026-02-12' AND last_seen_date < '2026-02-12'"
        ]);

        let incoming = IncomingEvent::new("model.my_project.my_model", "execute", &incoming_args);
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        let result = engine.validate(&incoming, &recorded);
        assert!(
            matches!(result, ValidationResult::Match),
            "Expected match after date literal sanitization, got {:?}",
            result
        );
    }

    #[test]
    fn test_sanitization_makes_double_quoted_date_sql_match() {
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args = serde_json::json!([
            r#"OPTIMIZE db.schema.tbl WHERE partition_date = "2026-02-17" ZORDER BY (game_id, puuid)"#
        ]);
        let recorded_args = serde_json::json!([
            r#"OPTIMIZE db.schema.tbl WHERE partition_date = "2026-02-11" ZORDER BY (game_id, puuid)"#
        ]);

        let incoming = IncomingEvent::new("model.my_project.my_model", "execute", &incoming_args);
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        let result = engine.validate(&incoming, &recorded);
        assert!(
            matches!(result, ValidationResult::Match),
            "Expected match after double-quoted date sanitization, got {:?}",
            result
        );
    }

    #[test]
    fn test_query_tag_sanitizer() {
        let sanitizer = QueryTagSanitizer;
        let sql = r#"alter session set query_tag = '{"app": "dbt", "dbt_snowflake_query_tags_version": "2.5.0", "is_incremental": false, "thread_id": "Thread-2"}'"#;
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(sanitized, "alter session set query_tag = ''");
    }

    #[test]
    fn test_query_tag_sanitizer_case_insensitive() {
        let sanitizer = QueryTagSanitizer;
        let sql = r#"ALTER SESSION SET QUERY_TAG = '{"thread_id": "Thread-5"}'"#;
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(sanitized, "ALTER SESSION SET QUERY_TAG = ''");
    }

    #[test]
    fn test_uuid_sanitizer() {
        let sanitizer = UuidSanitizer;
        let sql = "INSERT INTO t (id) VALUES ('019c48a8-c782-7921-8711-85c75a61128c')";
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(sanitized, "INSERT INTO t (id) VALUES ('<UUID>')");
    }

    #[test]
    fn test_uuid_sanitizer_multiple() {
        let sanitizer = UuidSanitizer;
        let sql = "VALUES ('019c48a8-c782-7921-8711-85c75a61128c',current_timestamp(),current_timestamp(),'PROD01.MODEL.person_master_bridge_tmp','STARTED')";
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(
            sanitized,
            "VALUES ('<UUID>',current_timestamp(),current_timestamp(),'PROD01.MODEL.person_master_bridge_tmp','STARTED')"
        );
    }

    #[test]
    fn test_uuid_sanitizer_case_insensitive() {
        let sanitizer = UuidSanitizer;
        let sql = "VALUES ('019C48A8-C782-7921-8711-85C75A61128C')";
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(sanitized, "VALUES ('<UUID>')");
    }

    #[test]
    fn test_sanitization_makes_uuid_sql_match() {
        let engine = TimeMachineEventValidationEngine::new();

        // SQL with different UUIDs
        let incoming_args = serde_json::json!([
            "insert into PROD01_SSOT_IDS.MODEL.ssot_dbt_audit_log values ('019c4dc8-1cb3-7bc2-bddf-21f86d6c4ac3',current_timestamp(),current_timestamp(),'PROD01_SSOT_IDS.MODEL.person_master_bridge_tmp','STARTED'); commit;"
        ]);
        let recorded_args = serde_json::json!([
            "insert into PROD01_SSOT_IDS.MODEL.ssot_dbt_audit_log values ('019c48a8-c782-7921-8711-85c75a61128c',current_timestamp(),current_timestamp(),'PROD01_SSOT_IDS.MODEL.person_master_bridge_tmp','STARTED'); commit;"
        ]);

        let incoming = IncomingEvent::new(
            "model.SSOT_DBT.person_master_bridge_tmp",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        let result = engine.validate(&incoming, &recorded);
        assert!(
            matches!(result, ValidationResult::Match),
            "Expected match after UUID sanitization, got {:?}",
            result
        );
    }

    #[test]
    fn test_whitespace_sanitizer() {
        let sanitizer = WhitespaceSanitizer;
        // Simulates the actual diff: "copy grants as (" vs "copy grants\n\n\n  as ("
        let sql1 = "  copy grants as (";
        let sql2 = "  copy grants\n\n\n  as (";
        assert_eq!(sanitizer.sanitize(sql1), sanitizer.sanitize(sql2));
        assert_eq!(sanitizer.sanitize(sql2), "copy grants as (");
    }

    #[test]
    fn test_whitespace_sanitizer_multiline() {
        let sanitizer = WhitespaceSanitizer;
        let sql = "SELECT\n    a,\n    b\nFROM\n    t";
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(sanitized, "SELECT a, b FROM t");
    }

    #[test]
    fn test_sql_comment_sanitizer_strips_line_comment_diagnostics() {
        let sanitizer = SqlCommentSanitizer;
        let a = "SELECT 1 WHERE x = 1 -- force_state: 2026-03-20T21:00:14.457688+00:00";
        let b = "SELECT 1 WHERE x = 1 -- force_state: 2026-03-25T17:25:42.672017+00:00";
        assert_eq!(sanitizer.sanitize(a), sanitizer.sanitize(b));
    }

    #[test]
    fn test_sql_comment_sanitizer_strips_block_comment() {
        let sanitizer = SqlCommentSanitizer;
        let a = "SELECT /* old */ 1";
        let b = "SELECT /* new */ 1";
        assert_eq!(sanitizer.sanitize(a), sanitizer.sanitize(b));
        assert_eq!(sanitizer.sanitize(a), "SELECT 1");
    }

    #[test]
    fn test_sql_comment_sanitizer_preserves_dash_dash_inside_string_literal() {
        let sanitizer = SqlCommentSanitizer;
        let sql = "SELECT '-- not a comment'";
        assert_eq!(sanitizer.sanitize(sql), sql);
    }

    #[test]
    fn test_engine_matches_when_only_sql_comment_differs() {
        let engine = TimeMachineEventValidationEngine::new();
        let incoming_args = serde_json::json!([
            "SELECT 1 FROM t WHERE a = 1 -- force_state: 2026-03-20T21:00:14+00:00"
        ]);
        let recorded_args = serde_json::json!([
            "SELECT 1 FROM t WHERE a = 1 -- force_state: 2026-03-25T17:25:42+00:00"
        ]);
        let incoming = IncomingEvent::new("model.p.my_model", "execute", &incoming_args);
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);
        let result = engine.validate(&incoming, &recorded);
        assert!(
            matches!(result, ValidationResult::Match),
            "Expected match when only line comments differ, got {:?}",
            result
        );
    }

    #[test]
    fn test_format_sql_for_display() {
        // Normalized SQL should be reformatted with newlines at keywords
        let normalized = "SELECT a, b FROM t WHERE x = 1 AND y = 2 ORDER BY a";
        let formatted = format_sql_for_display(normalized);
        assert!(formatted.contains('\n'), "Should have newlines");
        assert!(formatted.contains("\nFROM"), "Should break at FROM");
        assert!(formatted.contains("\nWHERE"), "Should break at WHERE");
        assert!(formatted.contains("\nAND"), "Should break at AND");
        assert!(formatted.contains("\nORDER BY"), "Should break at ORDER BY");
    }

    #[test]
    fn test_sanitization_makes_sql_match() {
        let engine = TimeMachineEventValidationEngine::new();

        // SQL with different timestamps
        let incoming_args = serde_json::json!([
            "INSERT INTO t (id, ts) VALUES ('019be853-9daf-7c50-b02f-369e65f08b69', '2026-01-23T00:49:47.760170')"
        ]);
        let recorded_args = serde_json::json!([
            "INSERT INTO t (id, ts) VALUES ('019be853-9daf-7c50-b02f-369e65f08b69', '2026-01-21T01:04:12.066273')"
        ]);

        let incoming = IncomingEvent::new("model.my_project.my_model", "execute", &incoming_args);
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        let result = engine.validate(&incoming, &recorded);
        assert!(
            matches!(result, ValidationResult::Match),
            "Expected match after sanitization, got {:?}",
            result
        );
    }

    #[test]
    fn test_whitespace_diff_matches_but_preserves_formatting_in_display() {
        let engine = TimeMachineEventValidationEngine::new();

        // SQL that differs only in whitespace (like the "copy grants as (" case)
        let incoming_args = serde_json::json!(["  copy grants as ("]);
        let recorded_args = serde_json::json!(["  copy grants\n\n\n  as ("]);

        let incoming = IncomingEvent::new("model.my_project.my_model", "execute", &incoming_args);
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        // Should match because whitespace is normalized for comparison
        let result = engine.validate(&incoming, &recorded);
        assert!(
            matches!(result, ValidationResult::Match),
            "Expected match after whitespace normalization, got {:?}",
            result
        );
    }

    #[test]
    fn test_uuid_sanitizer_double_quoted() {
        let sanitizer = UuidSanitizer;
        let sql = r#"select * /* --ELEMENTARY-METADATA-- {"command": "seed", "invocation_id": "019c577a-d8fd-7951-87d7-3424221a5afd"} */ order by 1"#;
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(
            sanitized,
            r#"select * /* --ELEMENTARY-METADATA-- {"command": "seed", "invocation_id": "<UUID>"} */ order by 1"#
        );
    }

    #[test]
    fn test_uuid_sanitizer_mixed_quotes() {
        let sanitizer = UuidSanitizer;
        let sql = r#"VALUES ('019c48a8-c782-7921-8711-85c75a61128c') /* "019c577a-d8fd-7951-87d7-3424221a5afd" */"#;
        let sanitized = sanitizer.sanitize(sql);
        assert_eq!(sanitized, r#"VALUES ('<UUID>') /* "<UUID>" */"#);
    }

    #[test]
    fn test_grant_ordering_sanitizer() {
        let sanitizer = GrantOrderingSanitizer;
        let sql1 = "grant USAGE on schema A to role R1; grant USAGE on schema B to role R2;";
        let sql2 = "grant USAGE on schema B to role R2; grant USAGE on schema A to role R1;";
        assert_ne!(sanitizer.sanitize(sql1), sanitizer.sanitize(sql2));
    }

    #[test]
    fn test_grant_ordering_sanitizer_same_target_revoke_batch() {
        let sanitizer = GrantOrderingSanitizer;
        let sql1 = "revoke DELETE on DB.SCH.tbl from ROLE_A; revoke SELECT on DB.SCH.tbl from ROLE_A; grant ALL on DB.SCH.tbl to ROLE_A;";
        let sql2 = "revoke SELECT on DB.SCH.tbl from ROLE_A; revoke DELETE on DB.SCH.tbl from ROLE_A; grant ALL on DB.SCH.tbl to ROLE_A;";
        assert_eq!(sanitizer.sanitize(sql1), sanitizer.sanitize(sql2));
    }

    #[test]
    fn test_grant_ordering_sanitizer_preserves_grant_revoke_group_order() {
        let sanitizer = GrantOrderingSanitizer;
        let sql1 = "revoke SELECT on DB.SCH.tbl from ROLE_A; grant ALL on DB.SCH.tbl to ROLE_A;";
        let sql2 = "grant ALL on DB.SCH.tbl to ROLE_A; revoke SELECT on DB.SCH.tbl from ROLE_A;";
        assert_ne!(sanitizer.sanitize(sql1), sanitizer.sanitize(sql2));
    }

    #[test]
    fn test_grant_ordering_sanitizer_preserves_non_grant_sql() {
        let sanitizer = GrantOrderingSanitizer;
        let sql = "SELECT 1; INSERT INTO t VALUES (1);";
        assert_eq!(sanitizer.sanitize(sql), sql);
    }

    #[test]
    fn test_grant_ordering_sanitizer_single_statement() {
        let sanitizer = GrantOrderingSanitizer;
        let sql = "grant USAGE on schema A to role R1";
        assert_eq!(sanitizer.sanitize(sql), sql);
    }

    #[test]
    fn test_elementary_invocation_id_sanitization() {
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args = serde_json::json!([
            r#"select artifacts_model, metadata_hash from "CLIENT_PROD"."ELEMENTARY"."DBT_ARTIFACTS_HASHES" order by metadata_hash /* --ELEMENTARY-METADATA-- {"command": "seed", "invocation_id": "019c7250-8418-7bd2-9ff8-a68f99684a3b"} --END-ELEMENTARY-METADATA-- */"#
        ]);
        let recorded_args = serde_json::json!([
            r#"select artifacts_model, metadata_hash from "CLIENT_PROD"."ELEMENTARY"."DBT_ARTIFACTS_HASHES" order by metadata_hash /* --ELEMENTARY-METADATA-- {"command": "seed", "invocation_id": "019c577a-d8fd-7951-87d7-3424221a5afd"} --END-ELEMENTARY-METADATA-- */"#
        ]);

        let incoming = IncomingEvent::new(
            "operation.elementary.elementary-on_run_end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        // ElementaryDeviation skips validation entirely for elementary nodes
        let result = engine.validate(&incoming, &recorded);
        assert!(
            matches!(result, ValidationResult::Skipped(_)),
            "Expected Skipped for elementary node, got {:?}",
            result
        );
    }

    #[test]
    fn test_mismatch_display_preserves_formatting() {
        let engine = TimeMachineEventValidationEngine::new();

        // SQL that has actual differences (not just whitespace)
        let incoming_args = serde_json::json!(["SELECT\n    a,\n    b\nFROM t"]);
        let recorded_args = serde_json::json!(["SELECT\n    x,\n    y\nFROM t"]);

        let incoming = IncomingEvent::new("model.my_project.my_model", "execute", &incoming_args);
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        let result = engine.validate(&incoming, &recorded);
        match result {
            ValidationResult::Mismatch(mismatch) => {
                // Display should preserve newlines for readable diff
                assert!(
                    mismatch.expected.contains('\n'),
                    "Expected display SQL to preserve newlines, got: {}",
                    mismatch.expected
                );
                assert!(
                    mismatch.actual.contains('\n'),
                    "Actual display SQL to preserve newlines, got: {}",
                    mismatch.actual
                );
            }
            _ => panic!("Expected mismatch, got {:?}", result),
        }
    }

    #[test]
    fn test_timestamp_sanitizer_space_separated() {
        let sanitizer = TimestampSanitizer;

        // Space-separated format used by elementary's DBT_INVOCATIONS
        let sql = "values ('abc','2026-02-13 14:49:54','2026-02-13 14:50:29')";
        let expected = "values ('abc','<TIMESTAMP>','<TIMESTAMP>')";
        assert_eq!(sanitizer.sanitize(sql), expected);

        // T-separated still works
        let sql = "values ('abc','2026-02-13T14:49:54','2026-02-13T14:50:29')";
        let expected = "values ('abc','<TIMESTAMP>','<TIMESTAMP>')";
        assert_eq!(sanitizer.sanitize(sql), expected);

        // Double-quoted space-separated
        let sql = r#"values ("abc","2026-02-13 14:49:54")"#;
        let expected = r#"values ("abc","<TIMESTAMP>")"#;
        assert_eq!(sanitizer.sanitize(sql), expected);
    }

    #[test]
    fn test_dbt_artifacts_deviation_skips_dbt_artifacts_nodes() {
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args = serde_json::json!(["INSERT INTO artifacts_table VALUES (1)"]);
        let recorded_args = serde_json::json!(["INSERT INTO artifacts_table VALUES (2)"]);

        let incoming = IncomingEvent::new(
            "operation.dbt_artifacts.dbt_artifacts-on_run_end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        assert!(matches!(
            engine.validate(&incoming, &recorded),
            ValidationResult::Skipped(_)
        ));
    }

    #[test]
    fn test_dbt_artifacts_deviation_skips_consuming_project_on_run_end_hook() {
        // dbt_artifacts registers on-run-end hooks under the consuming project's namespace,
        // so the node_id won't contain ".dbt_artifacts." — detect via INSERT INTO targeting
        // a schema whose name contains dbt_artifacts. Since PR #10448 the hook node_id uses
        // hyphens (`-on-run-end-`), which is the form the runtime now emits.
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_sql = "insert into `proj`.`myproject_dbt_artifacts`.`model_executions` ( node_id ) select col1 from values ( 'abc' )";
        let recorded_sql = "insert into `proj`.`myproject_dbt_artifacts`.`model_executions` ( node_id ) select col1 from values ( 'xyz' )";
        let incoming_args = serde_json::json!([incoming_sql]);
        let recorded_args = serde_json::json!([recorded_sql]);

        let incoming = IncomingEvent::new(
            "operation.myproject.myproject-on-run-end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        assert!(matches!(
            engine.validate(&incoming, &recorded),
            ValidationResult::Skipped(_)
        ));
    }

    #[test]
    fn test_dbt_artifacts_deviation_skips_consuming_project_legacy_underscore_hook() {
        // Older recordings (pre PR #10448) use the underscore form `-on_run_end-`. Keep
        // matching it so those recordings continue to replay.
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_sql = "insert into `proj`.`myproject_dbt_artifacts`.`model_executions` ( node_id ) select col1 from values ( 'abc' )";
        let recorded_sql = "insert into `proj`.`myproject_dbt_artifacts`.`model_executions` ( node_id ) select col1 from values ( 'xyz' )";
        let incoming_args = serde_json::json!([incoming_sql]);
        let recorded_args = serde_json::json!([recorded_sql]);

        let incoming = IncomingEvent::new(
            "operation.myproject.myproject-on_run_end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        assert!(matches!(
            engine.validate(&incoming, &recorded),
            ValidationResult::Skipped(_)
        ));
    }

    #[test]
    fn test_dbt_artifacts_deviation_does_not_skip_non_dbt_artifacts_on_run_end_hook() {
        // An on_run_end hook that does NOT target a dbt_artifacts schema should not be skipped.
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args =
            serde_json::json!(["insert into `proj`.`myschema`.`audit` values ( 1 )"]);
        let recorded_args =
            serde_json::json!(["insert into `proj`.`myschema`.`audit` values ( 2 )"]);

        let incoming = IncomingEvent::new(
            "operation.myproject.myproject-on_run_end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        assert!(matches!(
            engine.validate(&incoming, &recorded),
            ValidationResult::Mismatch(_)
        ));
    }

    #[test]
    fn test_dbt_artifacts_deviation_does_not_skip_other_nodes() {
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args = serde_json::json!(["INSERT INTO t VALUES (1)"]);
        let recorded_args = serde_json::json!(["INSERT INTO t VALUES (2)"]);

        let incoming = IncomingEvent::new(
            "operation.client.client-on_run_end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        assert!(matches!(
            engine.validate(&incoming, &recorded),
            ValidationResult::Mismatch(_)
        ));
    }

    #[test]
    fn test_elementary_deviation_skips_elementary_nodes() {
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args = serde_json::json!(["INSERT INTO elem_table VALUES (1)"]);
        let recorded_args = serde_json::json!(["INSERT INTO elem_table VALUES (2)"]);

        let incoming = IncomingEvent::new(
            "operation.elementary.elementary-on_run_end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        assert!(matches!(
            engine.validate(&incoming, &recorded),
            ValidationResult::Skipped(_)
        ));
    }

    #[test]
    fn test_elementary_deviation_does_not_skip_other_nodes() {
        let engine = TimeMachineEventValidationEngine::new();

        let incoming_args = serde_json::json!(["INSERT INTO t VALUES (1)"]);
        let recorded_args = serde_json::json!(["INSERT INTO t VALUES (2)"]);

        let incoming = IncomingEvent::new(
            "operation.client.client-on_run_end-0",
            "execute",
            &incoming_args,
        );
        let recorded = make_recorded_event(incoming.node_id, "execute", recorded_args);

        assert!(matches!(
            engine.validate(&incoming, &recorded),
            ValidationResult::Mismatch(_)
        ));
    }

    #[test]
    fn test_is_known_nondeterministic_node() {
        let engine = TimeMachineEventValidationEngine::new();

        assert!(
            engine.is_known_nondeterministic_node("operation.elementary.elementary-on_run_end-0")
        );
        assert!(
            engine.is_known_nondeterministic_node("operation.elementary.elementary-on_run_start-0")
        );
        assert!(!engine.is_known_nondeterministic_node("operation.client.client-on_run_end-0"));
        assert!(!engine.is_known_nondeterministic_node("model.my_project.my_model"));
    }
}
