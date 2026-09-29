use std::{collections::HashMap, sync::LazyLock};

use crate::AdapterType;
use chrono::{DateTime, NaiveDateTime};
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_frontend_common::Dialect;
use dbt_sql_utils::{SqlToken as DialectToken, sql_lex_tokens, sql_split_statements};

use super::tokenizer::{AbstractToken, Token, abstract_tokenize, tokenize};
use regex::Regex;

/// Normalize a pair of Python model payloads (the compiled `submit_python_job` code) so that
/// semantically equivalent but textually different forms compare equal. Reuses the same Python
/// canonicalizers `compare_sql` applies, in the same order. The main divergence this handles is
/// dbt-autofix's `CustomKeyInConfigDeprecation` rewrite, which turns `dbt.config.get(<custom key>)`
/// into `dbt.config.meta_get(...)` and moves the key from `config_dict` to `meta_dict`; both forms
/// resolve the same value at runtime. Unlike `compare_sql`, this does no SQL parsing, so it is safe
/// to run on raw Python model code.
pub fn canonicalize_python_model_pair(actual: &str, expected: &str) -> (String, String) {
    let actual = actual.replace("\r\n", "\n");
    let expected = expected.replace("\r\n", "\n");
    let actual = canonicalize_python_config_dict(&actual, &expected);
    let actual = canonicalize_python_meta_get_calls(&actual);
    let expected = canonicalize_python_meta_get_calls(&expected);
    let actual = canonicalize_python_meta_dict(&actual);
    let expected = canonicalize_python_meta_dict(&expected);
    (actual, expected)
}

/// Compare two SQL strings using deviation and canonicalization checks before strict comparison,
/// using adapter-specific canonicalization where applicable.
pub fn compare_sql(actual: &str, expected: &str, adapter_type: AdapterType) -> AdapterResult<()> {
    // Canonicalize ignorable differences first
    let actual = canonicalize_query_tag(actual);
    let expected = canonicalize_query_tag(expected);
    let actual = canonicalize_to_json_string_struct_field_order(&actual);
    let expected = canonicalize_to_json_string_struct_field_order(&expected);
    let actual = canonicalize_last_value_projection_order(&actual);
    let expected = canonicalize_last_value_projection_order(&expected);
    let actual = canonicalize_struct_projection_order_in_select_lists(&actual);
    let expected = canonicalize_struct_projection_order_in_select_lists(&expected);
    let actual = canonicalize_snowplow_context_identifier_projection_order(&actual);
    let expected = canonicalize_snowplow_context_identifier_projection_order(&expected);
    let actual = canonicalize_typographic_quotes_in_dollar_quoted_strings(&actual);
    let expected = canonicalize_typographic_quotes_in_dollar_quoted_strings(&expected);
    let actual = canonicalize_uuid_literals(&actual);
    let expected = canonicalize_uuid_literals(&expected);
    let actual = canonicalize_dbt_version_literal(&actual);
    let expected = canonicalize_dbt_version_literal(&expected);
    let actual = canonicalize_compact_timestamp_predicate(&actual, adapter_type);
    let expected = canonicalize_compact_timestamp_predicate(&expected, adapter_type);
    let actual = canonicalize_run_started_at_literal(&actual);
    let expected = canonicalize_run_started_at_literal(&expected);
    let actual = canonicalize_yyyymmdd_batch_literals(&actual);
    let expected = canonicalize_yyyymmdd_batch_literals(&expected);
    let actual = canonicalize_uuid_prefixed_test_unique_id_literals(&actual);
    let expected = canonicalize_uuid_prefixed_test_unique_id_literals(&expected);
    let actual = canonicalize_dbt_test_unique_id_literals(&actual);
    let expected = canonicalize_dbt_test_unique_id_literals(&expected);
    let actual = canonicalize_quoted_timestamp_space_separator(&actual);
    let expected = canonicalize_quoted_timestamp_space_separator(&expected);
    let actual = canonicalize_elementary_tmp_suffix(&actual);
    let expected = canonicalize_elementary_tmp_suffix(&expected);
    let actual = canonicalize_test_temp_relation_identifiers(&actual);
    let expected = canonicalize_test_temp_relation_identifiers(&expected);
    let actual = canonicalize_dbt_model_tmp_suffix(&actual);
    let expected = canonicalize_dbt_model_tmp_suffix(&expected);
    let actual = canonicalize_dbt_backup_timestamp_suffix(&actual);
    let expected = canonicalize_dbt_backup_timestamp_suffix(&expected);
    let actual = canonicalize_elementary_metadata_pkg_version(&actual);
    let expected = canonicalize_elementary_metadata_pkg_version(&expected);
    let actual = canonicalize_python_config_dict(&actual, &expected);
    // Apply meta_get→get normalization to both sides: Mantle recordings vary in
    // whether they preserve `dbt.config.meta_get(...)` or rewrite it to
    // `dbt.config.get(...)`, and Fusion preserves the user's source verbatim.
    // The two are semantically equivalent (the generated `config` class routes
    // both lookups through the same dict), so we normalize them on both sides.
    let actual = canonicalize_python_meta_get_calls(&actual);
    let expected = canonicalize_python_meta_get_calls(&expected);
    let actual = canonicalize_python_meta_dict(&actual);
    let expected = canonicalize_python_meta_dict(&expected);
    let actual = canonicalize_databricks_legacy_alter_column_comment_to_modern(&actual);
    let expected = canonicalize_databricks_legacy_alter_column_comment_to_modern(&expected);
    let actual = canonicalize_snowflake_grant_select_to_roles(&actual);
    let expected = canonicalize_snowflake_grant_select_to_roles(&expected);
    let actual = canonicalize_privilege_statement_order(&actual);
    let expected = canonicalize_privilege_statement_order(&expected);
    let actual = canonicalize_numeric_to_decimal(&actual);
    let expected = canonicalize_numeric_to_decimal(&expected);
    let actual = canonicalize_alter_table_set_tblproperties_order(&actual);
    let expected = canonicalize_alter_table_set_tblproperties_order(&expected);
    let actual = canonicalize_alter_set_tags_order(&actual);
    let expected = canonicalize_alter_set_tags_order(&expected);

    // Short-circuit: Elementary-generated SQL is allowed to drift across recorders/runners.
    // We only short-circuit when BOTH sides are clearly Elementary-originated.
    if is_elementary_query(&actual) && is_elementary_query(&expected) {
        return Ok(());
    }

    // Databricks/Spark: dbt tmp view definitions may differ in TEMPORARY vs non-temporary and
    // qualified vs unqualified view naming across runners/recorders. We treat these as equivalent
    // only for dbt tmp relations, and only when BOTH sides match the pattern.
    if matches!(adapter_type, AdapterType::Databricks | AdapterType::Spark) {
        if let (Some(actual_canon), Some(expected_canon)) = (
            canonicalize_databricks_tmp_view_definition(&actual),
            canonicalize_databricks_tmp_view_definition(&expected),
        ) {
            // Avoid recursion loops: only re-compare if we actually changed something.
            if actual_canon != actual || expected_canon != expected {
                if compare_sql(&actual_canon, &expected_canon, adapter_type).is_ok() {
                    return Ok(());
                }
            }
        }

        // Same pattern for MERGE ... USING: the __dbt_tmp relation may be qualified on one side
        // and unqualified on the other.
        if let (Some(actual_canon), Some(expected_canon)) = (
            canonicalize_databricks_tmp_merge_using(&actual),
            canonicalize_databricks_tmp_merge_using(&expected),
        ) {
            if actual_canon != actual || expected_canon != expected {
                if compare_sql(&actual_canon, &expected_canon, adapter_type).is_ok() {
                    return Ok(());
                }
            }
        }
    }

    // Heuristic: treat queries as equal if they only differ by a top-level
    // "select * from ( ... )" wrapper and benign CTE boundary syntax.
    if are_equivalent_ignoring_select_wrapper(&actual, &expected) {
        return Ok(());
    }

    // Create normalized SQL strings (remove all whitespace)
    let actual_normalized = actual
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let expected_normalized = expected
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    // In addition, remove trailing comments /* ... */ in both actual and expected
    let actual_normalized = if actual_normalized.ends_with("*/") {
        if let Some(last_comment_start) = actual_normalized.rfind("/*") {
            actual_normalized[..last_comment_start].to_string()
        } else {
            actual_normalized
        }
    } else {
        actual_normalized
    };
    let expected_normalized = if expected_normalized.ends_with("*/") {
        if let Some(last_comment_start) = expected_normalized.rfind("/*") {
            expected_normalized[..last_comment_start].to_string()
        } else {
            expected_normalized
        }
    } else {
        expected_normalized
    };

    // Ignore a single trailing semicolon (statement terminator). Some recorders include it
    // while others omit it for otherwise identical single statements.
    let actual_normalized = actual_normalized
        .strip_suffix(';')
        .unwrap_or(&actual_normalized);
    let expected_normalized = expected_normalized
        .strip_suffix(';')
        .unwrap_or(&expected_normalized);

    // Direct comparison first
    if actual_normalized == expected_normalized {
        return Ok(());
    }

    // fuzzy comparison
    if fuzzy_compare_sql(&actual, &expected) {
        return Ok(());
    }

    // lightweight structural comparison (includes CTE normalization)
    if compare_sql_structurally(&actual, &expected, adapter_type) {
        return Ok(());
    }

    // SQL differs, generate visual diff information
    let diff_info = generate_visual_sql_diff(&actual, &expected);

    Err(AdapterError::new(
        AdapterErrorKind::SqlMismatch,
        format!("SQL mismatch detected:\n\n{diff_info}"),
    ))
}

/// Canonicalize BigQuery-style `to_json_string(struct(...))` by sorting the STRUCT arguments
/// when they are simple identifiers.
///
/// We keep this intentionally narrow to avoid masking meaningful semantic differences:
/// - Only rewrites occurrences of `to_json_string(struct(<args>))` (case-insensitive).
/// - Only rewrites when every arg is a simple identifier / dotted identifier (optionally backticked),
///   with no whitespace or nested expressions.
fn canonicalize_to_json_string_struct_field_order(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("to_json_string") || !lower.contains("struct") {
        return sql.to_string();
    }

    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0usize;

    while i < sql.len() {
        // Find next "to_json_string" (case-insensitive) starting at i.
        let next = lower[i..].find("to_json_string");
        let Some(rel) = next else {
            out.push_str(&sql[i..]);
            break;
        };
        let start = i + rel;
        out.push_str(&sql[i..start]);

        // Advance past keyword.
        let mut j = start + "to_json_string".len();
        j = skip_ws(sql, j);
        if j >= sql.len() || bytes[j] as char != '(' {
            // Not a call site; keep literal text and continue scanning after start.
            out.push_str(&sql[start..j.min(sql.len())]);
            i = j;
            continue;
        }

        let to_open = j;
        let Some(to_close) = find_matching_paren(sql, to_open) else {
            out.push_str(&sql[start..]);
            break;
        };

        // Inside to_json_string(...)
        let mut k = to_open + 1;
        k = skip_ws(sql, k);
        if !starts_with_ci(sql, k, "struct") {
            out.push_str(&sql[start..=to_close]);
            i = to_close + 1;
            continue;
        }
        k += "struct".len();
        k = skip_ws(sql, k);
        if k >= sql.len() || bytes[k] as char != '(' {
            out.push_str(&sql[start..=to_close]);
            i = to_close + 1;
            continue;
        }

        let struct_open = k;
        let Some(struct_close) = find_matching_paren(sql, struct_open) else {
            out.push_str(&sql[start..=to_close]);
            i = to_close + 1;
            continue;
        };

        // Ensure struct_close aligns with the end of to_json_string(...) (only whitespace in between).
        let mut after_struct = struct_close + 1;
        after_struct = skip_ws(sql, after_struct);
        if after_struct != to_close {
            out.push_str(&sql[start..=to_close]);
            i = to_close + 1;
            continue;
        }

        let args_raw = &sql[struct_open + 1..struct_close];
        let args_raw = strip_sql_comments(args_raw);
        let Some(mut args) = split_top_level_commas(&args_raw) else {
            out.push_str(&sql[start..=to_close]);
            i = to_close + 1;
            continue;
        };
        if args.is_empty() || !args.iter().all(|a| is_simple_identifier_like(a)) {
            out.push_str(&sql[start..=to_close]);
            i = to_close + 1;
            continue;
        }

        args.sort_by_key(|a| a.to_ascii_lowercase());

        // Reconstruct: keep original casing of function names by copying from sql.
        // We normalize the struct args only; whitespace is irrelevant for compare_sql anyway.
        out.push_str(&sql[start..struct_open + 1]);
        out.push_str(&args.join(" , "));
        out.push_str(&sql[struct_close..=to_close]);
        i = to_close + 1;
    }

    out
}

#[derive(Clone, Debug)]
struct SqlScanner<'a> {
    sql: &'a str,
    bytes: &'a [u8],
    i: usize,
    depth_paren: usize,
    in_single: bool,
    in_double: bool,
    in_backtick: bool,
    in_line_comment: bool,
    in_block_comment: bool,
}

impl<'a> SqlScanner<'a> {
    fn new(sql: &'a str) -> Self {
        Self {
            sql,
            bytes: sql.as_bytes(),
            i: 0,
            depth_paren: 0,
            in_single: false,
            in_double: false,
            in_backtick: false,
            in_line_comment: false,
            in_block_comment: false,
        }
    }

    fn is_code(&self) -> bool {
        !self.in_single
            && !self.in_double
            && !self.in_backtick
            && !self.in_line_comment
            && !self.in_block_comment
    }

    fn set_pos(&mut self, i: usize) {
        self.i = i;
    }

    fn depth_paren(&self) -> usize {
        self.depth_paren
    }

    fn bump_one(&mut self) {
        if self.i >= self.sql.len() {
            return;
        }
        let ch = self.sql[self.i..]
            .chars()
            .next()
            .expect("i is on char boundary");
        let ch_len = ch.len_utf8();

        // Inside comment/quotes: only look for terminators.
        if self.in_line_comment {
            if ch == '\n' {
                self.in_line_comment = false;
            }
            self.i += ch_len;
            return;
        }
        if self.in_block_comment {
            if ch == '*' && self.i + 1 < self.bytes.len() && self.bytes[self.i + 1] as char == '/' {
                self.in_block_comment = false;
                self.i += 2;
            } else {
                self.i += ch_len;
            }
            return;
        }
        if self.in_single {
            if ch == '\\' && self.i + 1 < self.bytes.len() {
                self.i += 2;
            } else {
                if ch == '\'' {
                    self.in_single = false;
                }
                self.i += ch_len;
            }
            return;
        }
        if self.in_double {
            if ch == '\\' && self.i + 1 < self.bytes.len() {
                self.i += 2;
            } else {
                if ch == '"' {
                    self.in_double = false;
                }
                self.i += ch_len;
            }
            return;
        }
        if self.in_backtick {
            if ch == '`' {
                self.in_backtick = false;
            }
            self.i += ch_len;
            return;
        }

        // Code: detect comment/quote starts.
        if ch == '-' && self.i + 1 < self.bytes.len() && self.bytes[self.i + 1] as char == '-' {
            self.in_line_comment = true;
            self.i += 2;
            return;
        }
        if ch == '/' && self.i + 1 < self.bytes.len() && self.bytes[self.i + 1] as char == '*' {
            self.in_block_comment = true;
            self.i += 2;
            return;
        }
        if ch == '\'' {
            self.in_single = true;
            self.i += ch_len;
            return;
        }
        if ch == '"' {
            self.in_double = true;
            self.i += ch_len;
            return;
        }
        if ch == '`' {
            self.in_backtick = true;
            self.i += ch_len;
            return;
        }

        match ch {
            '(' => self.depth_paren += 1,
            ')' => self.depth_paren = self.depth_paren.saturating_sub(1),
            _ => {}
        }

        self.i += ch_len;
    }

    fn find_keyword_ci_at_depth(
        &mut self,
        lower: &str,
        kw: &str,
        target_depth: usize,
    ) -> Option<usize> {
        while self.i < self.sql.len() {
            if self.is_code()
                && self.depth_paren == target_depth
                && keyword_at(lower, self.i, kw, self.bytes)
            {
                return Some(self.i);
            }
            self.bump_one();
        }
        None
    }

    fn find_keyword_ci(&mut self, lower: &str, kw: &str) -> Option<usize> {
        while self.i < self.sql.len() {
            if self.is_code() && keyword_at(lower, self.i, kw, self.bytes) {
                return Some(self.i);
            }
            self.bump_one();
        }
        None
    }
}

/// Canonicalize SELECT projection lists that contain forward-fill style expressions:
///
///   LAST_VALUE(<col> IGNORE NULLS) OVER (<window>) AS <col>
///
/// When `<col>` comes from an unordered Jinja set/list, its order can drift across recorders/runners,
/// even when the query semantics are otherwise identical. We treat the projection order drift as
/// ignorable for replay, but keep this intentionally narrow:
///
/// - Only rewrites when there are 2+ matching `LAST_VALUE(..) OVER (.. ) AS ..` items in the same
///   SELECT list.
/// - Only rewrites when the `<col>` and the `AS <col>` alias are simple identifier-like tokens.
/// - Only rewrites when all matching items share an identical window spec (after whitespace
///   normalization).
/// - Only reorders the matching items; all other projection items remain in place.
fn canonicalize_last_value_projection_order(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("last_value") || !lower.contains("select") || !lower.contains("over") {
        return sql.to_string();
    }

    fn reorder_select_projection(projection: &str) -> Option<String> {
        let projection = projection.trim();
        let items = split_top_level_commas(projection)?;

        let mut candidates: Vec<(usize, String, String)> = Vec::new(); // (idx, alias_key, window_key)
        for (idx, item) in items.iter().enumerate() {
            if let Some((alias_key, window_key)) = parse_last_value_item(item) {
                candidates.push((idx, alias_key, window_key));
            }
        }
        if candidates.len() < 2 {
            return None;
        }
        let window0 = candidates[0].2.clone();
        if candidates.iter().any(|c| c.2 != window0) {
            return None;
        }

        let mut sorted: Vec<(String, String)> = candidates
            .iter()
            .map(|(idx, alias_key, _)| (alias_key.clone(), items[*idx].clone()))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));

        let mut new_items = items.clone();
        let mut idxs: Vec<usize> = candidates.iter().map(|c| c.0).collect();
        idxs.sort_unstable();
        for ((_, item), idx) in sorted.into_iter().zip(idxs) {
            new_items[idx] = item;
        }

        if new_items == items {
            return None;
        }
        Some(new_items.join(" , "))
    }

    let mut scanner = SqlScanner::new(sql);
    let mut out = String::with_capacity(sql.len());
    let mut emit = 0usize;

    while let Some(select_start) = scanner.find_keyword_ci(&lower, "select") {
        let base_depth = scanner.depth_paren();
        let after_select = select_start + "select".len();

        let mut from_scanner = scanner.clone();
        from_scanner.set_pos(after_select);
        let Some(from_pos) = from_scanner.find_keyword_ci_at_depth(&lower, "from", base_depth)
        else {
            break;
        };

        let projection = &sql[after_select..from_pos];
        if let Some(new_projection) = reorder_select_projection(projection) {
            out.push_str(&sql[emit..after_select]);
            if !new_projection.is_empty() {
                if !out.ends_with(char::is_whitespace) {
                    out.push(' ');
                }
                out.push_str(new_projection.trim());
                out.push(' ');
            }
            emit = from_pos;
        }

        // Continue scanning after the FROM keyword to avoid repeatedly matching the same SELECT.
        scanner = from_scanner;
        scanner.set_pos(from_pos + "from".len());
    }

    if emit == 0 {
        sql.to_string()
    } else {
        out.push_str(&sql[emit..]);
        out
    }
}

/// Canonicalize SELECT projection ordering drift for `STRUCT(...) AS <alias>` items.
///
/// Some Jinja/adapter code paths build a list of Snowplow context projections by iterating a map.
/// Different runners/recorders may emit identical projections in different orders (e.g. insertion
/// order vs key-sorted iteration). This is semantically irrelevant for a SELECT list, but replay
/// treats it as a SQL mismatch.
///
/// We keep this intentionally narrow:
/// - Only rewrites within `SELECT ... FROM` projection lists (same-paren-depth scan).
/// - Only considers items that begin with `STRUCT(` and end with `AS <simple_identifier>`.
/// - Only rewrites when there are 2+ such items in the same projection list.
/// - Only reorders the matching items; all other projection items remain in place.
fn canonicalize_struct_projection_order_in_select_lists(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("struct") || !lower.contains("select") {
        return sql.to_string();
    }

    fn struct_as_simple_alias(item: &str) -> Option<String> {
        let item = strip_sql_comments(item);
        let item = item.trim();
        if item.len() < "struct".len() + 1 {
            return None;
        }

        let struct_start: String = item.chars().take("struct".len()).collect();
        if !struct_start.eq_ignore_ascii_case("struct") {
            return None;
        }

        let chars: Vec<char> = item.chars().collect();

        let mut j = "struct".len();
        while j < chars.len() && chars[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= chars.len() || chars[j] != '(' {
            return None;
        }

        // Parse trailing `AS <alias>` from the end, avoiding any dependency on adapter-specific SQL.
        // We already removed comments and this item is a top-level SELECT projection (comma-split),
        // so a backwards parse is sufficient and avoids brittle regex.
        let mut end = chars.len();
        while end > 0 && chars[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        let mut start = end;
        while start > 0 {
            let b = chars[start - 1];
            if b.is_ascii_alphanumeric() || b == '_' || b == '.' || b == '`' {
                start -= 1;
            } else {
                break;
            }
        }
        if start == end {
            return None;
        }
        let alias: String = chars[start..end].iter().collect();
        let alias = alias.trim();
        if !is_simple_identifier_like(alias) {
            return None;
        }
        let mut k = start;
        while k > 0 && chars[k - 1].is_ascii_whitespace() {
            k -= 1;
        }
        if k < 2 {
            return None;
        }
        let as_start = k - 2;
        let as_word: String = chars[as_start..k].iter().collect();
        if !as_word.eq_ignore_ascii_case("as") {
            return None;
        }
        // Require `AS` to be a standalone word.
        if as_start > 0 && is_word_char(chars[as_start - 1]) {
            return None;
        }
        if k < chars.len() && is_word_char(chars[k]) {
            return None;
        }

        Some(alias.to_ascii_lowercase())
    }

    fn reorder_select_projection(projection: &str) -> Option<String> {
        let projection = projection.trim();
        // `split_top_level_commas` is not comment-aware; apostrophes inside `-- ...` comments can
        // incorrectly trip its quote tracking. Strip comments before splitting to keep this
        // canonicalizer robust on real-world SQL.
        let projection_clean = strip_sql_comments(projection);
        let items = split_top_level_commas(&projection_clean)?;

        let mut candidates: Vec<(usize, String)> = Vec::new(); // (idx, alias_key)
        for (idx, item) in items.iter().enumerate() {
            if let Some(alias_key) = struct_as_simple_alias(item) {
                candidates.push((idx, alias_key));
            }
        }
        if candidates.len() < 2 {
            return None;
        }

        let mut sorted: Vec<(String, String)> = candidates
            .iter()
            .map(|(idx, alias_key)| (alias_key.clone(), items[*idx].clone()))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));

        let mut new_items = items.clone();
        let mut idxs: Vec<usize> = candidates.iter().map(|c| c.0).collect();
        idxs.sort_unstable();
        for ((_, item), idx) in sorted.into_iter().zip(idxs) {
            new_items[idx] = item;
        }

        if new_items == items {
            return None;
        }
        Some(new_items.join(" , "))
    }

    let mut scanner = SqlScanner::new(sql);
    let mut out = String::with_capacity(sql.len());
    let mut emit = 0usize;

    while let Some(select_start) = scanner.find_keyword_ci(&lower, "select") {
        let base_depth = scanner.depth_paren();
        let after_select = select_start + "select".len();

        let mut from_scanner = scanner.clone();
        from_scanner.set_pos(after_select);
        let Some(from_pos) = from_scanner.find_keyword_ci_at_depth(&lower, "from", base_depth)
        else {
            break;
        };

        let projection = &sql[after_select..from_pos];
        if let Some(new_projection) = reorder_select_projection(projection) {
            out.push_str(&sql[emit..after_select]);
            if !new_projection.is_empty() {
                if !out.ends_with(char::is_whitespace) {
                    out.push(' ');
                }
                out.push_str(new_projection.trim());
                out.push(' ');
            }
            emit = from_pos;
        }

        scanner = from_scanner;
        scanner.set_pos(from_pos + "from".len());
    }

    if emit == 0 {
        sql.to_string()
    } else {
        out.push_str(&sql[emit..]);
        out
    }
}

/// Canonicalize SELECT projection ordering drift for plain Snowplow context identifier columns:
///
///   , experiment_entity
///   , feature_flag_context
///   , ...
///
/// These columns are produced earlier in the model as `STRUCT(...) AS <alias>`, and downstream
/// models often project them as simple identifiers. Different runners/recorders may emit them in
/// different orders due to map iteration semantics (insertion-order vs sorted iteration).
///
/// We keep this intentionally narrow:
/// - Only rewrites within `SELECT ... FROM` projection lists (same-paren-depth scan).
/// - Only considers items that are simple identifier-like tokens whose names end in `_entity`
///   or `_context`.
/// - Only rewrites when there are 2+ such items in the same projection list.
/// - Only reorders the matching items; all other projection items remain in place.
fn canonicalize_snowplow_context_identifier_projection_order(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("select") {
        return sql.to_string();
    }

    fn context_identifier_key(item: &str) -> Option<String> {
        let item = strip_sql_comments(item);
        let item = item.trim();
        if item.is_empty() {
            return None;
        }
        // Reject anything that looks like an expression.
        if item.contains('(')
            || item.contains(')')
            || item.contains('[')
            || item.contains(']')
            || item.contains('{')
            || item.contains('}')
        {
            return None;
        }
        // Reject explicit aliasing; those are handled by the STRUCT(...) AS <alias> canonicalizer.
        if item.to_ascii_lowercase().contains(" as ") {
            return None;
        }
        if !is_simple_identifier_like(item) {
            return None;
        }
        let ident = item.trim_matches('`');
        if !(ident.ends_with("_entity") || ident.ends_with("_context")) {
            return None;
        }
        Some(ident.to_ascii_lowercase())
    }

    fn reorder_select_projection(projection: &str) -> Option<String> {
        let projection = projection.trim();
        // Comments can contain apostrophes and confuse `split_top_level_commas`.
        let projection_clean = strip_sql_comments(projection);
        let items = split_top_level_commas(&projection_clean)?;

        let mut candidates: Vec<(usize, String)> = Vec::new(); // (idx, key)
        for (idx, item) in items.iter().enumerate() {
            if let Some(key) = context_identifier_key(item) {
                candidates.push((idx, key));
            }
        }
        if candidates.len() < 2 {
            return None;
        }

        let mut sorted: Vec<(String, String)> = candidates
            .iter()
            .map(|(idx, key)| (key.clone(), items[*idx].clone()))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));

        let mut new_items = items.clone();
        let mut idxs: Vec<usize> = candidates.iter().map(|c| c.0).collect();
        idxs.sort_unstable();
        for ((_, item), idx) in sorted.into_iter().zip(idxs) {
            new_items[idx] = item;
        }
        if new_items == items {
            return None;
        }
        Some(new_items.join(" , "))
    }

    let mut scanner = SqlScanner::new(sql);
    let mut out = String::with_capacity(sql.len());
    let mut emit = 0usize;

    while let Some(select_start) = scanner.find_keyword_ci(&lower, "select") {
        let base_depth = scanner.depth_paren();
        let after_select = select_start + "select".len();

        let mut from_scanner = scanner.clone();
        from_scanner.set_pos(after_select);
        let Some(from_pos) = from_scanner.find_keyword_ci_at_depth(&lower, "from", base_depth)
        else {
            break;
        };

        let projection = &sql[after_select..from_pos];
        if let Some(new_projection) = reorder_select_projection(projection) {
            out.push_str(&sql[emit..after_select]);
            if !new_projection.is_empty() {
                if !out.ends_with(char::is_whitespace) {
                    out.push(' ');
                }
                out.push_str(new_projection.trim());
                out.push(' ');
            }
            emit = from_pos;
        }

        scanner = from_scanner;
        scanner.set_pos(from_pos + "from".len());
    }

    if emit == 0 {
        sql.to_string()
    } else {
        out.push_str(&sql[emit..]);
        out
    }
}

/// Canonicalize Databricks/Spark dbt tmp view definitions that can be semantically equivalent for replay:
///
/// - `create or replace temporary view <name> as <query>`
/// - `create or replace view <db>.<schema>.<name> as <query>`
///
/// We keep this intentionally narrow:
/// - Only applies to `create or replace ... view ... as ...`
/// - Only applies when the view name's base identifier contains `__dbt_tmp`
/// - Strips leading `-- ...` line comments (e.g. replay-only divergence markers)
/// - Drops any qualification and normalizes to `create or replace temporary view <base> as <query>`
fn canonicalize_databricks_tmp_view_definition(sql: &str) -> Option<String> {
    // Fast-path: avoid regex work on the common case.
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("__dbt_tmp") || !lower.contains("view") || !lower.contains(" as") {
        return None;
    }

    static RE: LazyLock<Regex> = LazyLock::new(|| {
        // Note: we only strip *leading* line comments; we do not attempt full comment-aware parsing.
        Regex::new(
            r"(?is)^\s*(?:--[^\n]*\n\s*)*create\s+or\s+replace\s+(?:(?P<temp>temporary)\s+)?view\s+(?P<name>.+?)\s+as\s+(?P<body>.*)$",
        )
        .unwrap()
    });

    let caps = RE.captures(sql)?;
    let name = caps.name("name")?.as_str().trim();
    let body = caps.name("body")?.as_str();

    // Extract the base identifier from a possibly-qualified name like:
    //   `dbt`.`schema`.`sources__dbt_tmp`  -> `sources__dbt_tmp`
    //   sources__dbt_tmp                  -> sources__dbt_tmp
    // We treat dots as separators regardless of quoting (good enough for this narrow rule).
    let base = name
        .rsplit('.')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let base_unquoted = base
        .trim_matches('`')
        .trim_matches('"')
        .to_ascii_lowercase();
    if !base_unquoted.contains("__dbt_tmp") {
        return None;
    }

    Some(format!("create or replace temporary view {base} as {body}"))
}

/// Canonicalize Databricks/Spark MERGE statements where the `USING` clause may reference a
/// `__dbt_tmp` relation with different levels of qualification across runners:
///
/// - Fusion:  `merge into <target> ... using `db`.`schema`.`name__dbt_tmp` as ...`
/// - Mantle:  `merge into <target> ... using `name__dbt_tmp` as ...`
///
/// We normalize the `USING` relation to its unqualified base identifier (the last dot-segment)
/// when that identifier contains `__dbt_tmp`.
fn canonicalize_databricks_tmp_merge_using(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("__dbt_tmp") || !lower.contains("merge") || !lower.contains("using") {
        return None;
    }

    static RE: LazyLock<Regex> = LazyLock::new(|| {
        // Match the USING <relation> portion of a MERGE statement, allowing leading line comments.
        // The relation may be a multi-part quoted name like `db`.`schema`.`table__dbt_tmp`.
        Regex::new(
            r"(?is)(?P<prefix>^\s*(?:--[^\n]*\n\s*)*merge\s+into\s+.+?\s+using\s+)(?P<relation>(?:`[^`]+`\.)*`[^`]+`)(?P<suffix>\s+as\s+.*)$",
        )
        .unwrap()
    });

    let caps = RE.captures(sql)?;
    let relation = caps.name("relation")?.as_str().trim();

    // Extract the base identifier from a possibly-qualified name.
    let base = relation
        .rsplit('.')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let base_unquoted = base
        .trim_matches('`')
        .trim_matches('"')
        .to_ascii_lowercase();
    if !base_unquoted.contains("__dbt_tmp") {
        return None;
    }

    let prefix = caps.name("prefix")?.as_str();
    let suffix = caps.name("suffix")?.as_str();
    Some(format!("{prefix}{base}{suffix}"))
}

/// Canonicalize Databricks legacy column comment DDL to the modern COMMENT ON COLUMN syntax.
///
/// dbt-databricks may emit either of the following, which are semantically equivalent:
/// - `alter table <rel> change column <col> comment '<comment>'`
/// - `COMMENT ON COLUMN <rel>.<col> IS '<comment>'`
///
/// For replay diffs we treat them as equivalent by rewriting the legacy form into the modern form.
fn canonicalize_databricks_legacy_alter_column_comment_to_modern(sql: &str) -> String {
    // NOTE: We scope this extremely narrowly (anchored) to avoid masking unrelated DDL.
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?is)^\s*alter\s+table\s+(?P<rel>.+?)\s+change\s+column\s+(?P<col>`[^`]+`|[A-Za-z_][A-Za-z0-9_$]*)\s+comment\s+'(?P<comment>(?:\\'|''|[^'])*)'\s*;?\s*$"#,
        )
        .unwrap()
    });

    let Some(caps) = RE.captures(sql) else {
        return sql.to_string();
    };

    let rel = caps
        .name("rel")
        .expect("rel capture exists")
        .as_str()
        .trim();
    let comment = caps
        .name("comment")
        .expect("comment capture exists")
        .as_str();
    let mut col = caps
        .name("col")
        .expect("col capture exists")
        .as_str()
        .trim()
        .to_string();

    // If the relation uses backtick quoting and the column does not, add backticks around the column
    // to match the usual COMMENT ON COLUMN rendering.
    if rel.contains('`') && !col.contains('`') {
        col = format!("`{col}`");
    }

    format!("COMMENT ON COLUMN {rel}.{col} IS '{comment}'")
}

/// Canonicalize Snowflake GRANT SELECT statements where one side has an (invalid) python list
/// literal and the other side emits one statement per role.
///
/// Example (python list form):
/// - `grant select on DB.SCH.TBL to ['A', 'B'];`
///
/// Example (expanded form):
/// - `grant select on DB.SCH.TBL to A; grant select on DB.SCH.TBL to B;`
///
/// We only apply this rewrite when the entire SQL input consists exclusively of GRANT SELECT
/// statements (possibly separated by semicolons and whitespace). This keeps the heuristic narrow
/// and avoids masking unrelated DDL mismatches.
fn canonicalize_snowflake_grant_select_to_roles(sql: &str) -> String {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return sql.to_string();
    }

    let statements = sql_split_statements(trimmed, None);
    if statements.is_empty() {
        return sql.to_string();
    }

    // Match "grant select on <obj> to <to>"
    static GRANT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)^\s*grant\s+select\s+on\s+(?P<object>.+?)\s+to\s+(?P<to>.+?)\s*$"#)
            .unwrap()
    });

    let mut grants_by_object: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();

    for stmt in statements {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let Some(caps) = GRANT_RE.captures(stmt) else {
            // Not exclusively grant-select statements; do not rewrite.
            return sql.to_string();
        };

        let object = caps
            .name("object")
            .expect("object capture exists")
            .as_str()
            .trim()
            .to_string();
        let to_raw = caps.name("to").expect("to capture exists").as_str().trim();

        let roles = parse_grant_to_roles(to_raw).unwrap_or_else(|| {
            // Not a recognizable "to" payload; do not rewrite.
            Vec::new()
        });
        if roles.is_empty() {
            return sql.to_string();
        }

        let entry = grants_by_object.entry(object).or_default();
        for role in roles {
            entry.insert(role);
        }
    }

    if grants_by_object.is_empty() {
        return sql.to_string();
    }

    // Emit deterministically: objects sorted, roles sorted (BTreeSet).
    let mut objects: Vec<String> = grants_by_object.keys().cloned().collect();
    objects.sort();
    let mut out = String::new();
    for object in objects {
        let Some(roles) = grants_by_object.get(&object) else {
            continue;
        };
        for role in roles {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("grant select on {object} to {role};"));
        }
    }
    out
}

fn parse_grant_to_roles(to_raw: &str) -> Option<Vec<String>> {
    let mut s = to_raw.trim();
    // Snowflake syntax sometimes includes "ROLE"; tolerate it.
    if let Some(rest) = s.strip_prefix("role ") {
        s = rest.trim();
    } else if let Some(rest) = s
        .to_ascii_lowercase()
        .strip_prefix("role ")
        .and_then(|_| s.get(5..))
    {
        s = rest.trim();
    }

    // Python-list form: ['A', 'B']
    if s.starts_with('[') && s.ends_with(']') {
        let inner = s[1..s.len() - 1].trim();
        if inner.is_empty() {
            return None;
        }
        let mut roles = Vec::new();
        for part in inner.split(',') {
            let role = unquote_identifier_like(part.trim())?;
            roles.push(role);
        }
        return Some(roles);
    }

    // Single role form.
    Some(vec![unquote_identifier_like(s)?])
}

fn unquote_identifier_like(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // The only quoting we expect for this replay scenario is a Python string literal wrapper,
    // e.g. `'ANALYTICS'`. Strip exactly one matching pair of outer single quotes.
    let s = s
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(s);

    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrivilegeStatement {
    pub(crate) verb: String,
    pub(crate) privilege: String,
    pub(crate) object: String,
    pub(crate) principal: String,
}

impl PrivilegeStatement {
    pub(crate) fn render(&self) -> String {
        match self.verb.as_str() {
            "grant" => format!(
                "grant {} on {} to {};",
                self.privilege, self.object, self.principal
            ),
            "revoke" => format!(
                "revoke {} on {} from {};",
                self.privilege, self.object, self.principal
            ),
            _ => unreachable!("unexpected privilege statement verb"),
        }
    }
}

/// Canonicalize batches of semicolon-separated GRANT/REVOKE privilege statements by sorting
/// statements within contiguous verb groups.
///
/// This is intentionally narrow:
/// - every statement must be a simple `grant <priv> on <obj> to <principal>` or
///   `revoke <priv> on <obj> from <principal>`
/// - all statements must target the same object and principal
/// - the relative order of GRANT groups vs REVOKE groups is preserved
///
/// This lets replay ignore nondeterministic privilege iteration order without collapsing a
/// semantically meaningful revoke-then-grant sequence into grant-then-revoke.
fn canonicalize_privilege_statement_order(sql: &str) -> String {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return sql.to_string();
    }

    let statements = sql_split_statements(trimmed, None);
    if statements.len() < 2 {
        return sql.to_string();
    }

    static PRIVILEGE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?is)^\s*(?P<verb>grant|revoke)\s+(?P<privilege>.+?)\s+on\s+(?P<object>.+?)\s+(?P<direction>to|from)\s+(?P<principal>.+?)\s*$"#,
        )
        .unwrap()
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

        let verb = caps
            .name("verb")
            .expect("verb capture exists")
            .as_str()
            .trim()
            .to_ascii_lowercase();
        let direction = caps
            .name("direction")
            .expect("direction capture exists")
            .as_str()
            .trim()
            .to_ascii_lowercase();
        if (verb == "grant" && direction != "to") || (verb == "revoke" && direction != "from") {
            return sql.to_string();
        }

        parsed.push(PrivilegeStatement {
            verb,
            privilege: caps
                .name("privilege")
                .expect("privilege capture exists")
                .as_str()
                .trim()
                .to_string(),
            object: caps
                .name("object")
                .expect("object capture exists")
                .as_str()
                .trim()
                .to_string(),
            principal: caps
                .name("principal")
                .expect("principal capture exists")
                .as_str()
                .trim()
                .to_string(),
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

    out.join("\n")
}

/// Canonicalize `ALTER TABLE ... SET tblproperties (...)` by sorting the key-value
/// entries alphabetically by key. Databricks/Spark tblproperties are an unordered set
/// of key-value pairs, but Fusion and dbt-databricks may emit them in different order.
fn canonicalize_alter_table_set_tblproperties_order(sql: &str) -> String {
    // Match: ALTER TABLE <name> SET tblproperties (<entries>)
    // Anchored to the full statement to avoid masking unrelated DDL.
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)^(\s*ALTER\s+TABLE\s+.+?\s+SET\s+tblproperties\s*\()(.+?)(\)\s*)$")
            .unwrap()
    });

    let Some(caps) = RE.captures(sql) else {
        return sql.to_string();
    };

    let prefix = &caps[1]; // "ALTER TABLE ... SET tblproperties ("
    let entries_raw = &caps[2]; // "'key1' = 'val1' , 'key2' = 'val2' , ..."
    let suffix = &caps[3]; // ")"

    // Extract 'key' = 'value' pairs via regex to avoid breaking on commas inside quoted values.
    static ENTRY_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"'(?:[^'\\]|\\.)*'\s*=\s*'(?:[^'\\]|\\.)*'").unwrap());

    let mut entries: Vec<&str> = ENTRY_RE
        .find_iter(entries_raw)
        .map(|m| m.as_str())
        .collect();
    if entries.is_empty() {
        return sql.to_string();
    }
    entries.sort();

    format!("{}{}{}", prefix, entries.join(" , "), suffix)
}

/// Canonicalize `ALTER TABLE ... SET TAGS (...)` by sorting the key-value
/// entries alphabetically by key. dbt-databricks reads in `databricks_tags`
/// keys nondeterministically if they are set in inline `config` blocks, so
/// Fusion and dbt-databricks may emit them in a different order.
fn canonicalize_alter_set_tags_order(sql: &str) -> String {
    // Match: ALTER TABLE <name> SET TAGS (<entries>)
    // Anchored to the full statement to avoid masking unrelated DDL.
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)^(\s*ALTER\s+TABLE\s+.+?\s+SET\s+TAGS\s*\()(.+?)(\)\s*)$").unwrap()
    });

    let Some(caps) = RE.captures(sql) else {
        return sql.to_string();
    };

    let prefix = &caps[1]; // "ALTER TABLE ... SET TAGS ("
    let entries_raw = &caps[2]; // "'key1' = 'val1' , 'key2' = 'val2' , ..."
    let suffix = &caps[3]; // ")"

    // Extract 'key' = 'value' pairs via regex to avoid breaking on commas inside quoted values.
    static ENTRY_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"'(?:[^'\\]|\\.)*'\s*=\s*'(?:[^'\\]|\\.)*'").unwrap());

    let mut entries: Vec<&str> = ENTRY_RE
        .find_iter(entries_raw)
        .map(|m| m.as_str())
        .collect();
    if entries.is_empty() {
        return sql.to_string();
    }
    entries.sort();

    format!("{}{}{}", prefix, entries.join(" , "), suffix)
}

/// NUMERIC and DECIMAL are SQL-standard synonyms. Fusion may emit one while the
/// recording uses the other. Normalize `numeric(` → `decimal(` so comparisons succeed.
fn canonicalize_numeric_to_decimal(sql: &str) -> String {
    static NUMERIC_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bnumeric\s*\(").unwrap());
    NUMERIC_RE
        .replace_all(sql, |caps: &regex::Captures<'_>| {
            // Preserve original whitespace between "numeric" and "("
            let matched = &caps[0];
            let paren_idx = matched.find('(').unwrap();
            format!("decimal{}", &matched[paren_idx..])
        })
        .into_owned()
}

/// Lightweight structural comparator for SQL to relax overly strict mismatches.
/// Rules:
/// - Normalize whitespace significance by skipping it during parsing (inputs themselves are not mutated).
/// - Normalize redundant dbt-generated nested CTEs (where inner CTEs duplicate outer scope definitions).
/// - If both look like: `select * from (<subquery>) <rest>` then recursively compare both `<subquery>` and `<rest>`.
/// - Else, if both look like: `with n1 as (<sub1>), ..., nk as (<subk>) <sub>` then
///   ensure corresponding names match and recursively compare each `<subi>`, then compare `<sub>`.
/// - Else, if both look like a `union all` chain at top level, split into components,
///   sort components, and recursively compare pair-wise.
/// - All recursive comparisons call back into `compare_sql`.
fn compare_sql_structurally(actual: &str, expected: &str, adapter_type: AdapterType) -> bool {
    // Quick trims to reduce edge whitespace noise
    let a = actual.trim();
    let b = expected.trim();
    if a.is_empty() && b.is_empty() {
        return true;
    }

    // 1) select * from (<subquery>) <rest>
    if let (Some((a_sub, a_rest)), Some((b_sub, b_rest))) = (
        parse_select_star_from_parenthesized(a),
        parse_select_star_from_parenthesized(b),
    ) {
        return compare_sql(a_sub, b_sub, adapter_type).is_ok()
            && compare_sql(a_rest, b_rest, adapter_type).is_ok();
    }

    // 2) with n1 as (<sub1>), ..., nk as (<subk>) <sub>
    // Normalize redundant nested CTEs before structural comparison
    // This handles cases where one SQL has redundant nested CTE definitions
    // that match outer scope definitions, while the other reuses outer CTEs
    let a = canonicalize_redundant_nested_ctes(a);
    let b = canonicalize_redundant_nested_ctes(b);

    if let (Some((a_ctes, a_tail)), Some((b_ctes, b_tail))) =
        (parse_with_clause(&a), parse_with_clause(&b))
    {
        if a_ctes.len() != b_ctes.len() {
            return false;
        }
        for ((a_name, a_sql), (b_name, b_sql)) in a_ctes.iter().zip(b_ctes.iter()) {
            // Compare CTE names for equality (case-sensitive as a conservative choice)
            if a_name != b_name {
                return false;
            }
            if compare_sql(a_sql, b_sql, adapter_type).is_err() {
                return false;
            }
        }
        return compare_sql(a_tail, b_tail, adapter_type).is_ok();
    }

    // 3) CREATE [OR REPLACE] <stuff> AS (<subquery>)
    if let (Some((a_stuff, a_sub, a_keywords)), Some((b_stuff, b_sub, b_keywords))) =
        (parse_create_as_subquery(&a), parse_create_as_subquery(&b))
    {
        if a_keywords != b_keywords {
            return false;
        }
        return a_stuff == b_stuff && compare_sql(a_sub, b_sub, adapter_type).is_ok();
    }

    // 4) <sub1> union all <sub2> ... union all <sub_q>
    if let (Some(mut a_parts), Some(mut b_parts)) =
        (split_union_all_top_level(&a), split_union_all_top_level(&b))
    {
        if a_parts.len() > 1 && b_parts.len() > 1 && a_parts.len() == b_parts.len() {
            // Key-less lexicographic sort for deterministic pairing
            a_parts.sort();
            b_parts.sort();

            for (ax, bx) in a_parts.iter().zip(b_parts.iter()) {
                if compare_sql(ax, bx, adapter_type).is_err() {
                    return false;
                }
            }
            return true;
        }
    }

    // 4) <sub1> union <sub2> ... union <sub_q>
    if let (Some(mut a_parts), Some(mut b_parts)) =
        (split_union_top_level(&a), split_union_top_level(&b))
    {
        if a_parts.len() > 1 && b_parts.len() > 1 && a_parts.len() == b_parts.len() {
            // Key-less lexicographic sort for deterministic pairing
            a_parts.sort();
            b_parts.sort();

            for (ax, bx) in a_parts.iter().zip(b_parts.iter()) {
                if compare_sql(ax, bx, adapter_type).is_err() {
                    return false;
                }
            }
            return true;
        }
    }

    false
}

fn skip_ws(s: &str, mut i: usize) -> usize {
    let bytes = s.as_bytes();
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    i
}

fn is_word_char(b: char) -> bool {
    b.is_ascii_alphanumeric() || b == '_'
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn keyword_at(lower: &str, idx: usize, kw: &str, sql_bytes: &[u8]) -> bool {
    if !lower[idx..].starts_with(kw) {
        return false;
    }
    let end = idx + kw.len();
    let before_ok = idx == 0 || !is_word_byte(sql_bytes[idx.saturating_sub(1)]);
    let after_ok = end >= sql_bytes.len() || !is_word_byte(sql_bytes[end]);
    before_ok && after_ok
}

fn starts_with_ci(s: &str, i: usize, kw: &str) -> bool {
    s[i..]
        .to_ascii_lowercase()
        .starts_with(&kw.to_ascii_lowercase())
}

fn eat_keyword_ci(s: &str, mut i: usize, kw: &str) -> Option<usize> {
    if starts_with_ci(s, i, kw) {
        i += kw.len();
        Some(i)
    } else {
        None
    }
}

fn find_matching_paren(s: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in s.char_indices().skip(open_idx) {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn strip_sql_comments(s: &str) -> String {
    // Strip `-- ...` and `/* ... */` comments, but only when not inside quotes/backticks.
    // This is intentionally minimal and used only for replay canonicalization.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;

    while let Some(ch) = chars.next() {
        if in_single {
            out.push(ch);
            if ch == '\\' {
                // Preserve escaped char.
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            out.push(ch);
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if ch == '"' {
                in_double = false;
            }
            continue;
        }
        if in_backtick {
            out.push(ch);
            if ch == '`' {
                in_backtick = false;
            }
            continue;
        }

        // Not inside a quoted context.
        if ch == '\'' {
            in_single = true;
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_double = true;
            out.push(ch);
            continue;
        }
        if ch == '`' {
            in_backtick = true;
            out.push(ch);
            continue;
        }

        // Line comment.
        if ch == '-' && chars.peek() == Some(&'-') {
            chars.next(); // consume second '-'
            // Skip until newline, but keep the newline (if present) to avoid gluing tokens.
            if chars.any(|c| c == '\n') {
                out.push('\n');
            }
            continue;
        }

        // Block comment.
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume '*'
            loop {
                match chars.next() {
                    Some('*') if chars.peek() == Some(&'/') => {
                        chars.next(); // consume '/'
                        break;
                    }
                    None => break,
                    _ => {}
                }
            }
            continue;
        }

        out.push(ch);
    }

    out
}

fn is_simple_identifier_like(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    // Very conservative: allow alnum/underscore/dot/backtick only.
    // This supports `col`, `t.col`, and backticked identifiers.
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '`')
}

fn split_top_level_commas(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut depth_brace = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut escape_next = false;

    for (i, ch) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_single {
            if ch == '\\' {
                escape_next = true;
            } else if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '\\' {
                escape_next = true;
            } else if ch == '"' {
                in_double = false;
            }
            continue;
        }
        if in_backtick {
            if ch == '`' {
                in_backtick = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '`' => in_backtick = true,
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            '{' => depth_brace += 1,
            '}' => depth_brace = depth_brace.saturating_sub(1),
            ',' if depth_paren == 0 && depth_bracket == 0 && depth_brace == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }

    if in_single
        || in_double
        || in_backtick
        || depth_paren != 0
        || depth_bracket != 0
        || depth_brace != 0
    {
        return None;
    }
    out.push(s[start..].trim().to_string());
    Some(out.into_iter().filter(|x| !x.is_empty()).collect())
}

fn parse_last_value_item(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Allow comments inside the item (e.g. replay-only annotations).
    let s_clean = strip_sql_comments(s);
    let s = s_clean.trim();
    let bytes = s.as_bytes();
    let mut i = skip_ws(s, 0);
    i = eat_keyword_ci(s, i, "last_value")?;
    i = skip_ws(s, i);
    if i >= bytes.len() || bytes[i] as char != '(' {
        return None;
    }
    i += 1;
    i = skip_ws(s, i);
    let col_start = i;
    while i < bytes.len() && !(bytes[i] as char).is_whitespace() && bytes[i] as char != ')' {
        i += 1;
    }
    let col = s[col_start..i].trim();
    if !is_simple_identifier_like(col) {
        return None;
    }
    i = skip_ws(s, i);
    i = eat_keyword_ci(s, i, "ignore")?;
    i = skip_ws(s, i);
    i = eat_keyword_ci(s, i, "nulls")?;
    i = skip_ws(s, i);
    if i >= bytes.len() || bytes[i] as char != ')' {
        return None;
    }
    i += 1;
    i = skip_ws(s, i);
    i = eat_keyword_ci(s, i, "over")?;
    i = skip_ws(s, i);
    if i >= bytes.len() || bytes[i] as char != '(' {
        return None;
    }
    let window_open = i;
    let window_close = find_matching_paren(s, window_open)?;
    let window = &s[window_open..=window_close];
    let window_key = window
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();

    i = window_close + 1;
    i = skip_ws(s, i);
    i = eat_keyword_ci(s, i, "as")?;
    i = skip_ws(s, i);
    let alias_start = i;
    while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
        i += 1;
    }
    let alias = s[alias_start..i].trim();
    if !is_simple_identifier_like(alias) {
        return None;
    }
    if !col.eq_ignore_ascii_case(alias) {
        return None;
    }
    Some((alias.to_ascii_lowercase(), window_key))
}

fn parse_select_star_from_parenthesized(s: &str) -> Option<(&str, &str)> {
    // Recognize: select * from ( <subquery> ) <rest>
    let mut i = skip_ws(s, 0);
    i = eat_keyword_ci(s, i, "select")?;
    i = skip_ws(s, i);
    // Expect '*'
    let b = s.as_bytes();
    if i >= b.len() || b[i] as char != '*' {
        return None;
    }
    i += 1;
    i = skip_ws(s, i);
    i = eat_keyword_ci(s, i, "from")?;
    i = skip_ws(s, i);
    // Expect '('
    if i >= b.len() || b[i] as char != '(' {
        return None;
    }
    let open = i;
    let close = find_matching_paren(s, open)?;
    let sub = &s[open + 1..close];
    let rest = s[close + 1..].trim();
    Some((sub, rest))
}

fn parse_with_clause(s: &str) -> Option<(Vec<(String, String)>, &str)> {
    // Recognize: with n1 as (<sub1>), ..., nk as (<subk>) <tail>
    let mut i = skip_ws(s, 0);
    i = eat_keyword_ci(s, i, "with")?;
    let mut ctes: Vec<(String, String)> = Vec::new();
    let bytes = s.as_bytes();
    loop {
        i = skip_ws(s, i);
        // Parse CTE name up to 'as' (case-insensitive) that precedes '('
        let name_start = i;
        // Find 'as' while ensuring the following non-ws is '('
        let mut as_pos: Option<usize> = None;
        let mut j = i;
        while j < bytes.len() {
            // stop if we hit a top-level '(' before finding 'as' -> invalid for name
            if bytes[j] as char == '(' {
                break;
            }
            // try to match 'as'
            if starts_with_ci(s, j, "as") {
                // consume 'as' and any whitespace, then require '('
                let mut k = j + 2;
                k = skip_ws(s, k);
                if k < bytes.len() && bytes[k] as char == '(' {
                    as_pos = Some(j);
                    break;
                }
            }
            j += 1;
        }
        let as_pos = as_pos?;
        let name = s[name_start..as_pos].trim();
        if name.is_empty() {
            return None;
        }
        // Move to '('
        i = as_pos + 2;
        i = skip_ws(s, i);
        if i >= bytes.len() || bytes[i] as char != '(' {
            return None;
        }
        let open = i;
        let close = find_matching_paren(s, open)?;
        let sub = s[open + 1..close].trim().to_string();
        ctes.push((name.to_string(), sub));
        i = close + 1;
        i = skip_ws(s, i);
        if i < bytes.len() && bytes[i] as char == ',' {
            i += 1; // continue parsing next CTE
            continue;
        } else {
            // End of CTE list; the rest is the tail query
            let tail = s[i..].trim();
            return Some((ctes, tail));
        }
    }
}

/// Removes comments and trims start and end.
fn normalize_part(part: &str) -> String {
    strip_sql_comments(part).trim_start().trim().to_string()
}

fn split_union_top_level(s: &str) -> Option<Vec<String>> {
    // Split on top-level "union all" (case-insensitive)
    let mut parts: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let lower = s.to_ascii_lowercase();
    let mut iter = lower.char_indices().peekable();
    while let Some((i, ch)) = iter.next() {
        if ch == '(' {
            depth += 1;
            continue;
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 0 && lower[i..].starts_with("union") {
            // ensure it is "union"
            let k = i + "union".len();
            // require at least one whitespace
            let k_after_ws = skip_ws(&lower, k);
            if k_after_ws > k {
                // boundary found
                let left = &s[start..i];
                parts.push(normalize_part(left));
                let next_i = k_after_ws;
                start = next_i;
                while let Some(&(peek_i, _)) = iter.peek() {
                    if peek_i < next_i {
                        iter.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
        }
    }
    // push final segment
    let last = &s[start..];
    if !parts.is_empty() {
        parts.push(normalize_part(last));
        return Some(parts);
    }
    None
}

fn split_union_all_top_level(s: &str) -> Option<Vec<String>> {
    // Split on top-level "union all" (case-insensitive)
    let mut parts: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let lower = s.to_ascii_lowercase();
    let mut iter = lower.char_indices().peekable();
    while let Some((i, ch)) = iter.next() {
        if ch == '(' {
            depth += 1;
            continue;
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 0 && lower[i..].starts_with("union") {
            // ensure it is "union all"
            let k = i + "union".len();
            // require at least one whitespace
            let k_after_ws = skip_ws(&lower, k);
            if k_after_ws > k && lower[k_after_ws..].starts_with("all") {
                // boundary found
                let left = &s[start..i];
                parts.push(normalize_part(left));
                // advance past "union all"
                let next_i = k_after_ws + "all".len();
                start = next_i;
                while let Some(&(peek_i, _)) = iter.peek() {
                    if peek_i < next_i {
                        iter.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
        }
    }
    // push final segment
    let last = &s[start..];
    if !parts.is_empty() {
        parts.push(normalize_part(last));
        return Some(parts);
    }
    None
}

fn parse_create_as_subquery(s: &str) -> Option<(&str, &str, Vec<&str>)> {
    // Recognize: CREATE [OR REPLACE] <stuff> AS (<subquery>)  or  CREATE [OR REPLACE] <stuff> AS <subquery>
    // Case-insensitive for keywords; preserve exact <stuff> for equality check.
    //
    // The bare (unparenthesized) form matters because `AS (<subquery>)` and `AS <subquery>` are
    // always semantically identical SQL, and not every code path that emits a `CREATE ... AS`
    // statement wraps the body in parens -- e.g. dbt-core's hand-rolled `latest_version`
    // pointer-view SQL is a bare `create or replace view X as select ...`, while a parametrized
    // materialization macro might always wrap the body as `as (\n ... \n)`. This scan still
    // prefers a parenthesized match when one exists (identical to the original behavior), and
    // only falls back to the bare form when no `AS (` is found anywhere in the statement.
    let mut i = skip_ws(s, 0);
    i = eat_keyword_ci(s, i, "create")?;
    i = skip_ws(s, i);
    // Optional "or replace"
    if let Some(mut j) = eat_keyword_ci(s, i, "or") {
        j = skip_ws(s, j);
        if let Some(k) = eat_keyword_ci(s, j, "replace") {
            i = k;
        } // if "or" not followed by "replace", keep original i (treat as not present)
    }

    let mut keywords = Vec::new();

    i = skip_ws(s, i);
    if let Some(j) = eat_keyword_ci(s, i, "transient") {
        keywords.push("transient");
        i = skip_ws(s, j);
    } else if let Some(j) = eat_keyword_ci(s, i, "temporary") {
        keywords.push("temporary");
        i = skip_ws(s, j);
    }

    let stuff_start = i;
    // Find the wrapped form's 'as' followed by '(' (case-insensitive), not inside parentheses,
    // while also tracking the last bare, whole-word top-level 'as' as a fallback.
    let lower = s.to_ascii_lowercase();
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut depth = 0usize;
    let mut wrapped_as_pos: Option<usize> = None;
    let mut bare_as_pos: Option<usize> = None;
    for (j, ch) in lower.char_indices() {
        if j < i {
            continue;
        }
        if ch == '(' {
            depth += 1;
            continue;
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 0 && lower[j..].starts_with("as") {
            let prev_is_word = lower[..j].chars().next_back().is_some_and(is_word_char);
            let next_is_word = lower[j + 2..].chars().next().is_some_and(is_word_char);
            if prev_is_word || next_is_word {
                continue; // part of a longer identifier (e.g. "alias"), not the keyword
            }
            let mut k = j + 2;
            k = skip_ws(&lower, k);
            if k < lower.len() && (lower.as_bytes()[k] as char) == '(' {
                wrapped_as_pos = Some(j);
                break;
            }
            bare_as_pos = Some(j);
        }
    }

    if let Some(as_pos) = wrapped_as_pos {
        let stuff = s[stuff_start..as_pos].trim();
        let mut k = as_pos + 2;
        k = skip_ws(s, k);
        if k >= s.len() || s.as_bytes()[k] as char != '(' {
            return None;
        }
        let open = k;
        let close = find_matching_paren(s, open)?;
        let sub = s[open + 1..close].trim();
        return Some((stuff, sub, keywords));
    }

    let as_pos = bare_as_pos?;
    let stuff = s[stuff_start..as_pos].trim();
    let mut k = as_pos + 2;
    k = skip_ws(s, k);
    if k >= s.len() {
        return None;
    }
    let sub = s[k..].trim();
    let sub = sub.strip_suffix(';').unwrap_or(sub).trim();
    if sub.is_empty() {
        return None;
    }
    Some((stuff, sub, keywords))
}

/// Replace the payload of `ALTER SESSION SET QUERY_TAG = '...';` with a fixed placeholder,
/// so differences in the query tag JSON/body are ignored during comparison.
fn canonicalize_query_tag(sql: &str) -> String {
    // Match: ALTER SESSION SET QUERY_TAG = '...'
    // Flags: (?i) case-insensitive, (?s) allow '.' to match newlines (defensive)
    // We specifically capture a single-quoted literal to avoid over-matching.
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)\balter\s+session\s+set\s+query_tag\s*=\s*'[^']*'").unwrap()
    });
    RE.replace_all(sql, "alter session set query_tag = '__TAG__'")
        .to_string()
}

/// Replace single-quoted UUID string literals with a fixed `'UUID'` placeholder.
/// Example: '8f439b7e-752f-460a-8d1a-f469231d169c' -> 'UUID'
/// This is a blunt instrument. Ideally, we should address the problem at the root:
/// A lot of these are from {{ invocation_id }}. The value of the original invocation_id
/// is available in manifest.json. We should consider using it in replay. TODO: Do this!
fn canonicalize_uuid_literals(sql: &str) -> String {
    // Case-insensitive UUID regex inside single quotes. Tolerates whitespace between the quote
    // and the UUID (e.g. `' <uuid> '`), which some customer macros add around an
    // `invocation_id`-derived audit-column literal. Regression: fs#14150.
    static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)'\s*[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\s*'")
            .unwrap()
    });
    UUID_RE.replace_all(sql, "'UUID'").to_string()
}

/// Canonicalize Elementary metadata `dbt_pkg_version` drift for the Elementary metadata package only.
///
/// Elementary creates/updates a metadata table like:
/// `... analytics_elementary_metadata.metadata as (select '0.21.0' as dbt_pkg_version)`
///
/// The specific package version string isn't semantically meaningful for replay comparison, but
/// we scope this canonicalization narrowly to avoid masking legitimate literal differences.
fn canonicalize_elementary_metadata_pkg_version(sql: &str) -> String {
    // Fast-path: only touch likely Elementary `model.elementary.metadata` materializations.
    //
    // Projects can change where Elementary models land (database/schema), so we cannot rely on a
    // fixed schema/table prefix like `analytics_elementary_metadata.metadata`.
    //
    // Instead, we scope this canonicalization to DDL that creates a `... .metadata` table and
    // contains the `dbt_pkg_version` field literal.
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("dbt_pkg_version") {
        return sql.to_string();
    }
    // Must look like DDL for a table named `metadata`.
    // We keep this intentionally conservative to avoid masking unrelated literals.
    if !(lower.contains("create or replace")
        && lower.contains("table")
        && lower.contains(".metadata"))
    {
        return sql.to_string();
    }

    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)'[0-9]+(?:\.[0-9]+){2}'\s+as\s+dbt_pkg_version").unwrap()
    });
    RE.replace_all(sql, "'DBT_PKG_VERSION' as dbt_pkg_version")
        .to_string()
}

/// Case-insensitive substring check that avoids allocating a lowercased copy of `sql`.
/// `needle` must be lowercase ASCII and non-empty.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

// Replay-drift literal patterns: each matches a literal whose value is decided by the engine
// build or the run's wall clock, never by the user's source, plus the SQL context that identifies
// it. An alias or exact expression shape keeps these from rewriting unrelated literals.

/// Matches a digit-leading quoted semver-ish literal, an optional `::type` cast, and the
/// `as dbt_version` alias. The leading-digit requirement keeps it off unrelated string literals.
static DBT_VERSION_LITERAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)'[0-9][0-9A-Za-z.+_-]*'(?P<cast>::[a-z_][a-z0-9_]*)?\s+as\s+dbt_version")
        .unwrap()
});

/// Matches a quoted ISO-8601 timestamp (either `T` or space separator, optional `Z`/offset), an
/// optional `::type` cast, and the `as [dbt_]run_started_at` alias.
static RUN_STARTED_AT_LITERAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)'[0-9]{4}-[0-9]{2}-[0-9]{2}[T ][0-9:.]+(?:Z|[+-][0-9]{2}:?[0-9]{2})?'(?P<cast>::[a-z_][a-z0-9_]*)?\s+as\s+(?P<alias>dbt_run_started_at|run_started_at)",
    )
    .unwrap()
});

/// Replace a `dbt_version` literal with a fixed placeholder — the replay engine's version and
/// the recorded engine's version differ by construction.
/// Example: '2026.8.4+2f16d1c' as dbt_version -> 'DBT_VERSION' as dbt_version
fn canonicalize_dbt_version_literal(sql: &str) -> String {
    if !contains_ignore_ascii_case(sql, "dbt_version") {
        return sql.to_string();
    }
    DBT_VERSION_LITERAL_RE
        .replace_all(sql, "'DBT_VERSION'$cast as dbt_version")
        .to_string()
}

/// Replace a `run_started_at`/`dbt_run_started_at` literal with a fixed placeholder — it's
/// derived from the run's wall-clock time, so it only matches a recording under exact clock
/// pinning.
/// Example: '2026-08-09T16:01:19' as dbt_run_started_at -> 'RUN_STARTED_AT' as dbt_run_started_at
fn canonicalize_run_started_at_literal(sql: &str) -> String {
    if !contains_ignore_ascii_case(sql, "run_started_at") {
        return sql.to_string();
    }
    RUN_STARTED_AT_LITERAL_RE
        .replace_all(sql, "'RUN_STARTED_AT'$cast as $alias")
        .to_string()
}

/// Replace the upper bound of the reported Snowflake temporary-view timestamp predicate when it
/// equals the compact form of the statement's aliased `dbt_run_started_at` value.
fn canonicalize_compact_timestamp_predicate(sql: &str, adapter_type: AdapterType) -> String {
    match adapter_type {
        AdapterType::Snowflake => {}
        _ => return sql.to_string(),
    }
    if !contains_ignore_ascii_case(sql, "to_timestamp") {
        return sql.to_string();
    }

    let Some(tokens) = sql_lex_tokens(sql, Dialect::Snowflake) else {
        return sql.to_string();
    };
    let Some(view_body) = temporary_view_body_tokens(&tokens) else {
        return sql.to_string();
    };
    let Some(compact_run_started_at) = compact_dbt_run_started_at(view_body) else {
        return sql.to_string();
    };

    let mut rhs_spans = view_body
        .windows(14)
        .filter_map(|tokens| compact_timestamp_predicate_rhs(tokens, &compact_run_started_at))
        .collect::<Vec<_>>();
    if rhs_spans.is_empty() {
        return sql.to_string();
    }

    let mut canonical = sql.to_string();
    rhs_spans.sort_unstable();
    for (start, end) in rhs_spans.into_iter().rev() {
        canonical.replace_range(start..end, "'00000000000000'");
    }
    canonical
}

fn temporary_view_body_tokens(tokens: &[DialectToken]) -> Option<&[DialectToken]> {
    const PREFIX: [&str; 5] = ["create", "or", "replace", "temporary", "view"];
    if tokens.len() < PREFIX.len()
        || !tokens
            .iter()
            .zip(PREFIX)
            .all(|(token, expected)| token.text.eq_ignore_ascii_case(expected))
    {
        return None;
    }

    let as_index = tokens
        .iter()
        .enumerate()
        .skip(PREFIX.len())
        .find_map(|(index, token)| token.text.eq_ignore_ascii_case("as").then_some(index))?;
    let open_index = as_index + 1;
    if tokens.get(open_index)?.text != "(" {
        return None;
    }

    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        match token.text.as_str() {
            "(" => depth += 1,
            ")" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&tokens[open_index + 1..index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn compact_dbt_run_started_at(tokens: &[DialectToken]) -> Option<String> {
    tokens.windows(3).find_map(|tokens| {
        if !tokens[1].text.eq_ignore_ascii_case("as")
            || !tokens[2].text.eq_ignore_ascii_case("dbt_run_started_at")
        {
            return None;
        }
        compact_iso_timestamp(single_quoted_literal(&tokens[0])?)
    })
}

fn compact_iso_timestamp(value: &str) -> Option<String> {
    let normalized = value.replacen(' ', "T", 1);
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(timestamp.format("%Y%m%d%H%M%S").to_string());
    }
    NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|timestamp| timestamp.format("%Y%m%d%H%M%S").to_string())
}

fn compact_timestamp_predicate_rhs(
    tokens: &[DialectToken],
    compact_run_started_at: &str,
) -> Option<(usize, usize)> {
    let punctuation = [
        (2, "("),
        (4, ","),
        (6, ")"),
        (7, "<"),
        (9, "("),
        (11, ","),
        (13, ")"),
    ];
    if !tokens[0].text.eq_ignore_ascii_case("where")
        || !tokens[1].text.eq_ignore_ascii_case("to_timestamp")
        || !tokens[8].text.eq_ignore_ascii_case("to_timestamp")
        || !punctuation
            .iter()
            .all(|(index, expected)| tokens[*index].text == *expected)
    {
        return None;
    }

    let lhs = single_quoted_literal(&tokens[3])?;
    let lhs_format = single_quoted_literal(&tokens[5])?;
    let rhs = single_quoted_literal(&tokens[10])?;
    let rhs_format = single_quoted_literal(&tokens[12])?;
    if lhs.len() != 14
        || !lhs.bytes().all(|byte| byte.is_ascii_digit())
        || rhs != compact_run_started_at
        || !lhs_format.eq_ignore_ascii_case("YYYYMMDDHH24MISSFF3")
        || !rhs_format.eq_ignore_ascii_case("YYYYMMDDHH24MISSFF3")
    {
        return None;
    }
    Some((tokens[10].start, tokens[10].end))
}

fn single_quoted_literal(token: &DialectToken) -> Option<&str> {
    token.text.strip_prefix('\'')?.strip_suffix('\'')
}

/// Replace `YYYYMMDDHHMMSS`-derived batch id and archive path literals with fixed placeholders —
/// like `run_started_at`, these are wall-clock derived and only match under exact clock pinning.
/// Example: 20260808051943::number as etl_batch_id -> 00000000000000::number as etl_batch_id
/// Example: archive/20260808_180232/ -> archive/00000000_000000/
fn canonicalize_yyyymmdd_batch_literals(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    let mut out = sql.to_string();

    if lower.contains("etl_batch_id") {
        static BATCH_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)\b[0-9]{14}\b(?P<cast>::[a-z_][a-z0-9_]*)?\s+as\s+etl_batch_id")
                .unwrap()
        });
        out = BATCH_ID_RE
            .replace_all(&out, "00000000000000$cast as etl_batch_id")
            .to_string();
    }

    if lower.contains("archive/") {
        static ARCHIVE_PATH_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?i)(archive/)[0-9]{8}_[0-9]{6}(/)").unwrap());
        out = ARCHIVE_PATH_RE
            .replace_all(&out, "${1}00000000_000000${2}")
            .to_string();
    }

    out
}

/// Public because `compare_sql` treats *any* two Elementary-originated statements as equal, so a
/// caller that searches a recording for a specific statement needs to know when a match is that
/// wildcard rather than evidence it found the right statement.
pub fn is_elementary_query(sql: &str) -> bool {
    // Elementary appends an explicit marker comment to its generated SQL.
    // We treat any SQL containing this marker as Elementary-originated.
    sql.contains("--ELEMENTARY-METADATA--")
}

/// The prefix used by dbt for ephemeral model CTEs.
const DBT_CTE_PREFIX: &str = "__dbt__cte__";

/// Canonicalize SQL by removing redundant nested dbt CTEs that match definitions in scope.
///
/// This is necessary because mantle sporadically materializes unnecessary CTEs (potential
/// race condition in mantle) while fusion does not. This normalization allows comparing
/// SQL from both systems by treating redundant nested dbt CTEs as equivalent to referencing
/// the outer CTE that is already in scope.
///
/// This normalization only applies to CTEs with the `__dbt__cte__` prefix, which are
/// generated by dbt for ephemeral models. User-defined CTEs are not affected.
///
/// The normalization works recursively at arbitrary nesting levels. A nested dbt CTE is
/// considered redundant if it has the same name AND the same definition as a CTE
/// that is already in scope (from any parent level).
///
/// Example - these two are semantically equivalent:
/// ```sql
/// -- Query 1: with nested redundant dbt CTEs at multiple levels
/// with __dbt__cte__abc as (select a from A),
/// __dbt__cte__efg as (
///     with __dbt__cte__abc as (select a from A),  -- redundant, same as outer
///     __dbt__cte__hij as (select b from B),
///     __dbt__cte__lll as (
///         with __dbt__cte__hij as (select b from B)  -- redundant
///         select hij.b as b
///     )
///     select abc.a, hij.b, lll.b
/// )
/// select * from __dbt__cte__abc, __dbt__cte__efg
/// ```
/// ```sql
/// -- Query 2: reuses dbt CTEs from outer scopes
/// with __dbt__cte__abc as (select a from A),
/// __dbt__cte__efg as (
///     with __dbt__cte__hij as (select b from B),
///     __dbt__cte__lll as (select hij.b as b)
///     select abc.a, hij.b, lll.b
/// )
/// select * from __dbt__cte__abc, __dbt__cte__efg
/// ```
fn canonicalize_redundant_nested_ctes(sql: &str) -> String {
    // Start with an empty scope and process recursively
    let scope = HashMap::new();
    canonicalize_nested_ctes_with_scope(sql, &scope)
}

/// Recursively canonicalize CTEs with an accumulated scope of CTEs from parent levels.
///
/// The scope contains all CTEs that are "in scope" at this level - i.e., CTEs defined
/// at this level or any parent level that can be referenced without re-definition.
fn canonicalize_nested_ctes_with_scope(
    sql: &str,
    parent_scope: &HashMap<String, String>,
) -> String {
    // Parse CTEs at this level
    let Some((ctes, tail)) = parse_with_clause(sql) else {
        return sql.to_string();
    };

    // Build accumulated scope: start with parent scope
    let mut current_scope = parent_scope.clone();

    // First pass: add all CTE definitions at this level to the scope
    // (we need this for sibling CTE references)
    for (name, body) in &ctes {
        let normalized_body = normalize_sql_for_cte_comparison(body);
        current_scope.insert(name.to_lowercase(), normalized_body);
    }

    // Second pass: process each CTE body, removing redundant nested CTEs
    let mut new_ctes: Vec<(String, String)> = Vec::new();
    for (cte_name, cte_body) in ctes {
        let new_body = remove_redundant_nested_ctes_recursive(&cte_body, &current_scope);
        new_ctes.push((cte_name, new_body));
    }

    // Rebuild the SQL
    if new_ctes.is_empty() {
        tail.to_string()
    } else {
        let cte_parts: Vec<String> = new_ctes
            .iter()
            .map(|(name, body)| format!("{} as ({})", name, body))
            .collect();
        format!("with {} {}", cte_parts.join(", "), tail)
    }
}

/// Normalize SQL for CTE comparison (remove whitespace, lowercase).
fn normalize_sql_for_cte_comparison(sql: &str) -> String {
    sql.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// Check if a CTE name is a dbt-generated ephemeral CTE (has __dbt__cte__ prefix).
fn is_dbt_cte(name: &str) -> bool {
    name.to_lowercase()
        .starts_with(&DBT_CTE_PREFIX.to_lowercase())
}

/// Remove redundant nested dbt CTEs from a CTE body, recursively handling arbitrary nesting.
///
/// A nested dbt CTE is redundant if:
/// 1. Its name starts with `__dbt__cte__`
/// 2. It has the same name as a CTE in the current scope
/// 3. Its definition (after normalization) matches the in-scope CTE's definition
///
/// Non-dbt CTEs (without the prefix) are never considered redundant and are always kept.
/// All CTEs (dbt or not) are added to the scope for deeper nested levels.
fn remove_redundant_nested_ctes_recursive(
    body: &str,
    parent_scope: &HashMap<String, String>,
) -> String {
    // Try to parse as a WITH clause
    let Some((inner_ctes, inner_tail)) = parse_with_clause(body) else {
        return body.to_string();
    };

    // Build accumulated scope for this level
    let mut current_scope = parent_scope.clone();

    // Filter out inner dbt CTEs that match definitions in scope
    let mut keep_ctes: Vec<(String, String)> = Vec::new();
    for (inner_name, inner_body) in inner_ctes {
        let inner_name_lower = inner_name.to_lowercase();
        let inner_body_normalized = normalize_sql_for_cte_comparison(&inner_body);

        // Only consider dbt CTEs (with __dbt__cte__ prefix) for redundancy removal
        let is_redundant = if is_dbt_cte(&inner_name) {
            // Check if this dbt CTE matches one already in scope
            if let Some(scope_body) = parent_scope.get(&inner_name_lower) {
                scope_body == &inner_body_normalized
            } else {
                false
            }
        } else {
            // Non-dbt CTEs are never considered redundant
            false
        };

        if is_redundant {
            // Skip this redundant dbt CTE - it's already available in the parent scope
            continue;
        }

        // This CTE is not redundant - add it to scope for nested processing
        current_scope.insert(inner_name_lower, inner_body_normalized);

        // Recursively process nested CTEs in the body with the updated scope
        let processed_body = remove_redundant_nested_ctes_recursive(&inner_body, &current_scope);
        keep_ctes.push((inner_name, processed_body));
    }

    // Rebuild the body
    if keep_ctes.is_empty() {
        inner_tail.to_string()
    } else {
        let cte_parts: Vec<String> = keep_ctes
            .iter()
            .map(|(name, body)| format!("{} as ({})", name, body))
            .collect();
        format!("with {} {}", cte_parts.join(", "), inner_tail)
    }
}

/// Canonicalize string literals that embed `invocation_id` (UUID) + dbt test unique_id.
///
/// These often look like:
///   '<invocation_uuid>.test.<package>.<test_name>.<suffix>'
///
/// Mantle/Fusion can differ in invocation_id and in how they truncate/hash test names, but
/// the specific embedded literal is not semantically meaningful for replay comparison.
fn canonicalize_uuid_prefixed_test_unique_id_literals(sql: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.test\.[^']*'",
        )
        .unwrap()
    });
    RE.replace_all(sql, "'UUID.test.TEST_UNIQUE_ID'")
        .to_string()
}

/// Canonicalize dbt test node unique IDs embedded in string literals.
///
/// These look like:
///   'test.<package>.<test_name>.<suffix>'
///
/// This is intentionally narrow (must start with `test.` inside single quotes) to avoid
/// masking unrelated string literals.
fn canonicalize_dbt_test_unique_id_literals(sql: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)'test\.[^']*'").unwrap());
    RE.replace_all(sql, "'test.TEST_UNIQUE_ID'").to_string()
}

/// Canonicalize quoted timestamp literals that use a space separator between date and time.
///
/// This enables the existing fuzzy timestamp matcher (which expects `T`) to work for
/// patterns like:
///   '2025-12-23 07:06:03' -> '2025-12-23T07:06:03'
fn canonicalize_quoted_timestamp_space_separator(sql: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        // Narrow to single-quoted literals to avoid touching non-literal SQL fragments.
        // Supports optional fractional seconds and optional timezone suffix (e.g. +00:00 or Z).
        Regex::new(r"'(\d{4}-\d{2}-\d{2})\s+(\d{2}:\d{2}:\d{2}(?:\.\d+)?)(?:([+-]\d{2}:\d{2}|Z))?'")
            .unwrap()
    });
    // Use explicit braces to avoid `$1T` being interpreted as a (non-existent) capture.
    RE.replace_all(sql, "'${1}T${2}${3}'").to_string()
}

/// Replace dynamic tmp suffixes produced by some packages (e.g., elementary) that append
/// utc.now()-like timestamps to temporary table names, such as:
///   dbt_sources__tmp_20251203160139043240  ->  dbt_sources__tmp_TIMESTAMP
fn canonicalize_elementary_tmp_suffix(sql: &str) -> String {
    // Case-insensitive; match "__tmp_" followed by a long digit run (timestamps/unique suffixes)
    // Scope it to a plausible leading year 2000-2100 to avoid over-matching.
    // Example matched: "__tmp_20251203160139043240"
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(__tmp_)(?:20[0-9]{2}|2100)\d{8,}").unwrap());
    RE.replace_all(sql, "${1}TIMESTAMP").to_string()
}

/// Canonicalize dbt test temp relation identifiers that may differ between recorders/runners.
///
/// This is intentionally narrow:
/// - Only matches identifiers that start with `test_` and contain `__tmp_...`.
/// - Assumes `canonicalize_elementary_tmp_suffix` already normalized the timestamp portion to `TIMESTAMP`.
///
/// Example:
///   PROD.SCH.test_0f6b...__schema_baseline__tmp_TIMESTAMP
///   PROD.SCH.test_7a2c...__schema_baseline__tmp_TIMESTAMP
/// both become:
///   PROD.SCH.test_ALPHA__tmp_TIMESTAMP
fn canonicalize_test_temp_relation_identifiers(sql: &str) -> String {
    // Fast-path: avoid regex work on the common case.
    if !sql.to_ascii_lowercase().contains("__tmp_") {
        return sql.to_string();
    }

    static RE: LazyLock<Regex> = LazyLock::new(|| {
        // Replace the variable middle portion of `test_<...>__tmp_TIMESTAMP` with `ALPHA`.
        Regex::new(r"(?i)(\btest_)[0-9a-z_]+(__tmp_TIMESTAMP\b)").unwrap()
    });
    static QUOTED_RE: LazyLock<Regex> = LazyLock::new(|| {
        // When the identifier is quoted (e.g. Snowflake), the quotes become separate tokens and
        // can cause mismatches even after normalizing the middle portion. Strip quotes only for
        // canonical dbt test temp identifiers.
        Regex::new(r#"(?i)"(test_[0-9a-z_]+__tmp_TIMESTAMP)""#).unwrap()
    });

    let out = RE.replace_all(sql, "${1}ALPHA${2}");
    QUOTED_RE.replace_all(&out, "$1").to_string()
}

/// Canonicalize dbt model temporary table identifiers that may differ between recorders/runners.
///
/// Model materialization creates temporary tables with random numeric suffixes like:
///   stg_sfmc_open__dbt_tmp192731837139227
///   stg_sfmc_open__dbt_tmp073022229909
///
/// This function normalizes them to a canonical form to allow SQL replay comparison.
///
/// Example:
///   "stg_sfmc_open__dbt_tmp192731837139227" -> "stg_sfmc_open__dbt_tmpSUFFIX"
///   "stg_sfmc_open__dbt_tmp073022229909" -> "stg_sfmc_open__dbt_tmpSUFFIX"
fn canonicalize_dbt_model_tmp_suffix(sql: &str) -> String {
    // Fast-path: avoid regex work on the common case.
    if !sql.to_ascii_lowercase().contains("__dbt_tmp") {
        return sql.to_string();
    }

    static RE: LazyLock<Regex> = LazyLock::new(|| {
        // Match __dbt_tmp followed by one or more digits
        // This captures the pattern used by dbt for temporary table suffixes
        Regex::new(r"(?i)(__dbt_tmp_?)\d*\b").unwrap()
    });

    RE.replace_all(sql, "${1}SUFFIX").to_string()
}

/// Canonicalize backup relation identifiers whose suffix is derived from the wall clock, such as
/// the ones custom materializations build with `py_current_timestring()`:
///   ANALYTICS.SCH.orders_DBT_BACKUP_20260824102757911107255
///   ANALYTICS.SCH.orders__dbt_backup_20240101000000000000
/// both become `..._DBT_BACKUP_TIMESTAMP`.
///
/// The recorded suffix is fixed at record time and replay regenerates it, so the identifiers can
/// never match. This is intentionally narrow:
/// - Only matches a `_dbt_backup_` marker (case-insensitive) immediately followed by digits.
/// - Only matches a plausible `py_current_timestring()` value: a year in 2000-2100 followed by at
///   least 8 more digits (the format is `%Y%m%d%H%M%S%f`, so the shortest real value is 14 digits).
fn canonicalize_dbt_backup_timestamp_suffix(sql: &str) -> String {
    // Fast-path: avoid regex work on the common case.
    if !contains_ignore_ascii_case(sql, "_dbt_backup_") {
        return sql.to_string();
    }

    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(_dbt_backup_)(?:20[0-9]{2}|2100)[0-9]{8,}").unwrap());

    RE.replace_all(sql, "${1}TIMESTAMP").to_string()
}

/// Check whether two SQL strings are identical modulo a top-level
/// "select * from ( ... )" wrapper and CTE boundary differences.
fn are_equivalent_ignoring_select_wrapper(actual: &str, expected: &str) -> bool {
    let norm_actual = normalize_for_wrapper_diff(actual);
    let norm_expected = normalize_for_wrapper_diff(expected);
    if norm_actual == norm_expected {
        return false;
    }
    let cleaned_actual = canonicalize_cte_boundaries(remove_select_star_wrapper(&norm_actual));
    let cleaned_expected = canonicalize_cte_boundaries(remove_select_star_wrapper(&norm_expected));
    if cleaned_actual == cleaned_expected {
        return true;
    }
    // Ignore extra parentheses without an expensive diff
    remove_all_parens(&cleaned_actual) == remove_all_parens(&cleaned_expected)
}

fn normalize_for_wrapper_diff(sql: &str) -> String {
    // Remove line comments starting with -- to end of line
    let mut out = String::with_capacity(sql.len());
    for line in sql.lines() {
        if let Some(idx) = line.find("--") {
            out.push_str(&line[..idx]);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    // Collapse all whitespace and lowercase
    // Precompiled regex for performance
    static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    WS_RE.replace_all(&out, "").to_lowercase()
}

fn remove_select_star_wrapper(norm_sql: &str) -> String {
    // norm_sql is already lowercased and whitespace-free.
    const PATTERN: &str = "select*from(";
    if let Some(idx) = norm_sql.find(PATTERN) {
        let mut candidate = String::with_capacity(norm_sql.len());
        candidate.push_str(&norm_sql[..idx]);
        candidate.push_str(&norm_sql[idx + PATTERN.len()..]);
        while candidate.ends_with(')') {
            candidate.pop();
        }
        candidate
    } else {
        norm_sql.to_string()
    }
}

fn canonicalize_cte_boundaries(norm_sql: String) -> String {
    norm_sql.replace(")with", "),")
}

fn remove_all_parens(s: &str) -> String {
    s.replace(['(', ')'], "")
}

/// Normalize typographic quotes inside `$$...$$` dollar-quoted strings.
///
/// Some SQL generators/editors may emit unicode “smart quotes” within `COMMENT $$...$$`
/// clauses. For replay comparison purposes, we consider those equivalent to ASCII quotes.
fn canonicalize_typographic_quotes_in_dollar_quoted_strings(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    let mut in_dollar = false;

    while let Some(pos) = rest.find("$$") {
        let (segment, after_segment) = rest.split_at(pos);
        if in_dollar {
            out.extend(segment.chars().map(|c| match c {
                '\u{201C}' | '\u{201D}' => '"',
                _ => c,
            }));
        } else {
            out.push_str(segment);
        }
        out.push_str("$$");
        rest = &after_segment[2..];
        in_dollar = !in_dollar;
    }

    if in_dollar {
        out.extend(rest.chars().map(|c| match c {
            '\u{201C}' | '\u{201D}' => '"',
            _ => c,
        }));
    } else {
        out.push_str(rest);
    }

    out
}

/// For Python models, canonicalize config_dict when actual differs from expected only by extra meta keys.
/// This handles cases where project-level configs add extra meta keys that dbt-core doesn't serialize.
/// Only replaces the entire config_dict if:
/// 1. Actual's meta contains all expected meta keys with same values (superset)
/// 2. All non-meta keys match exactly between actual and expected
fn canonicalize_python_config_dict(actual: &str, expected: &str) -> String {
    // Check if both contain config_dict (Python model indicator)
    if !actual.contains("config_dict = {") || !expected.contains("config_dict = {") {
        return actual.to_string();
    }

    // Extract config_dict from both
    let actual_config_dict = extract_config_dict(actual);
    let expected_config_dict = extract_config_dict(expected);

    if actual_config_dict.is_none() || expected_config_dict.is_none() {
        return actual.to_string();
    }

    let (actual_dict_start, actual_dict_end, actual_dict_str) = actual_config_dict.unwrap();
    let (_expected_dict_start, _expected_dict_end, expected_dict_str) =
        expected_config_dict.unwrap();

    // Try to parse both as Python dicts
    if let (Ok(actual_dict), Ok(expected_dict)) = (
        parse_python_dict(&actual_dict_str),
        parse_python_dict(&expected_dict_str),
    ) {
        // Check if actual's meta is a superset of expected's meta
        if let (Some(actual_meta), Some(expected_meta)) = (
            actual_dict.get("meta").and_then(|v| v.as_dict()),
            expected_dict.get("meta").and_then(|v| v.as_dict()),
        ) {
            // Verify all expected meta keys are in actual and have the same value
            let meta_is_superset = expected_meta
                .iter()
                .all(|(k, v)| actual_meta.get(k) == Some(v));

            // Verify all non-meta keys in expected also exist in actual with same values
            // This ensures we only canonicalize when the dicts differ only in meta's extra keys
            let non_meta_keys_match = expected_dict
                .iter()
                .filter(|(k, _)| k.as_str() != "meta")
                .all(|(k, v)| actual_dict.get(k) == Some(v));

            if meta_is_superset && non_meta_keys_match {
                // Safe to canonicalize by replacing actual's config_dict with expected's
                let mut result = String::with_capacity(actual.len());
                result.push_str(&actual[..actual_dict_start]);
                result.push_str(&expected_dict_str);
                result.push_str(&actual[actual_dict_end..]);
                return result;
            }
        }
    }

    actual.to_string()
}

/// Canonicalize `dbt.config.meta_get(` call sites to `dbt.config.get(`.  Fusion passes the user's
/// Python source verbatim; Mantle recordings vary — some preserve `meta_get`, others rewrite to
/// `get`.  The two forms are semantically equivalent because the generated `config` class routes
/// all lookups through the same dict, so the caller applies this normalization to both sides.
fn canonicalize_python_meta_get_calls(sql: &str) -> String {
    if !sql.contains("dbt.config.meta_get(") {
        return sql.to_string();
    }
    sql.replace("dbt.config.meta_get(", "dbt.config.get(")
}

/// Strip the `meta_dict` variable and the `meta_get` static method that Fusion emits inside
/// the Python `config` helper class for Snowflake Python models.  Mantle does not emit these.
///
/// An empty `meta_dict = {}` is just removed. A populated one is different: Fusion puts
/// meta config values there and leaves `config_dict = {}` empty, but Mantle puts those same
/// values straight into `config_dict`. So we promote the meta_dict content into config_dict,
/// then delete the meta_dict line. Now both sides match.
fn canonicalize_python_meta_dict(sql: &str) -> String {
    // Fast-path: nothing to do when there is no meta_dict.
    if !sql.contains("meta_dict") {
        return sql.to_string();
    }

    // 1. Remove the standalone `meta_dict = {}` line (with surrounding blank lines collapsed).
    static RE_META_DICT_EMPTY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^\s*meta_dict\s*=\s*\{\}\s*\n").unwrap());

    // 2. Match a non-empty `meta_dict = {…}` line and capture the dict literal.
    //    The assignment is always on a single line; greedy [^\n]* followed by \} finds the last
    //    closing brace on that line, correctly handling nested dicts.
    static RE_META_DICT_NONEMPTY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^[ \t]*meta_dict\s*=\s*(\{[^\n]*\})\s*\n").unwrap());

    // 3. Remove the `meta_get` static method block inside the config class.
    //    Matches:
    //        @staticmethod
    //        def meta_get(key, default=None):
    //            return meta_dict.get(key, default)
    //    (with flexible indentation and an optional trailing blank line)
    static RE_META_GET_METHOD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
                r"(?m)^[ \t]*@staticmethod\s*\n[ \t]*def meta_get\(.*?\):\s*\n[ \t]*return meta_dict\.get\(.*?\)\s*\n?"
            )
            .unwrap()
    });

    let mut result = sql.to_string();

    // When config_dict is empty and meta_dict is populated, promote meta_dict → config_dict.
    if result.contains("config_dict = {}") {
        if let Some(caps) = RE_META_DICT_NONEMPTY.captures(&result.clone()) {
            if let Some(meta_content) = caps.get(1).map(|m| m.as_str()) {
                result = result.replacen(
                    "config_dict = {}",
                    &format!("config_dict = {}", meta_content),
                    1,
                );
            }
        }
    }

    let result = RE_META_DICT_EMPTY.replace_all(&result, "");
    let result = RE_META_DICT_NONEMPTY.replace_all(&result, "");
    let result = RE_META_GET_METHOD.replace_all(&result, "");
    result.into_owned()
}

fn extract_config_dict(sql: &str) -> Option<(usize, usize, String)> {
    let start_pattern = "config_dict = ";
    let start = sql.find(start_pattern)?;
    let dict_start = start + start_pattern.len();

    // Verify the next character is {
    if !sql[dict_start..].starts_with('{') {
        return None;
    }

    // Start scanning from after the opening {
    let scan_start = dict_start + 1;

    // Find the matching closing brace
    let mut depth = 0;
    let mut string_quote: Option<char> = None;
    let mut escape_next = false;
    let mut dict_end = dict_start;

    for (i, ch) in sql[scan_start..].chars().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' => escape_next = true,
            '\'' | '"' => {
                if let Some(quote) = string_quote {
                    // We're in a string, check if this closes it
                    if ch == quote {
                        string_quote = None;
                    }
                } else {
                    // We're not in a string, this opens one
                    string_quote = Some(ch);
                }
            }
            '{' if string_quote.is_none() => {
                depth += 1;
            }
            '}' if string_quote.is_none() => {
                if depth == 0 {
                    dict_end = scan_start + i + 1;
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }

    if dict_end == dict_start {
        return None;
    }

    Some((dict_start, dict_end, sql[dict_start..dict_end].to_string()))
}

/// Simple Python dict value representation
#[derive(Debug, Clone, PartialEq)]
enum PyValue {
    String(String),
    Dict(HashMap<String, PyValue>),
    None,
}

impl PyValue {
    fn as_dict(&self) -> Option<&HashMap<String, PyValue>> {
        match self {
            PyValue::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// Parse a simple Python dict literal (limited to what we need for config_dict)
fn parse_python_dict(s: &str) -> Result<HashMap<String, PyValue>, String> {
    let s = s.trim();
    if !s.starts_with('{') || !s.ends_with('}') {
        return Err("Not a dict".to_string());
    }

    let inner = &s[1..s.len() - 1].trim();
    if inner.is_empty() {
        return Ok(HashMap::new());
    }

    let mut result = HashMap::new();
    let mut remaining = *inner;

    while !remaining.is_empty() {
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }

        // Parse key (quoted string)
        if !remaining.starts_with('\'') && !remaining.starts_with('"') {
            return Err("Expected quoted key".to_string());
        }

        let quote_char = remaining.chars().next().unwrap();
        let key_end = remaining[1..].find(quote_char).ok_or("Unterminated key")?;
        let key = remaining[1..=key_end].to_string();
        remaining = remaining[key_end + 2..].trim_start();

        // Expect colon
        if !remaining.starts_with(':') {
            return Err("Expected colon".to_string());
        }
        remaining = remaining[1..].trim_start();

        // Parse value
        let (value, rest) = parse_python_value(remaining)?;
        result.insert(key, value);
        remaining = rest.trim_start();

        // Handle comma
        if remaining.starts_with(',') {
            remaining = &remaining[1..];
        }
    }

    Ok(result)
}

fn parse_python_value(s: &str) -> Result<(PyValue, &str), String> {
    let s = s.trim_start();

    if s.starts_with('{') {
        // Parse nested dict
        let mut depth = 0;
        let mut end = 0;
        let mut string_quote: Option<char> = None;
        let mut escape_next = false;

        for (i, ch) in s.chars().enumerate() {
            if escape_next {
                escape_next = false;
                continue;
            }

            match ch {
                '\\' => escape_next = true,
                '\'' | '"' => {
                    if let Some(quote) = string_quote {
                        // We're in a string, check if this closes it
                        if ch == quote {
                            string_quote = None;
                        }
                    } else {
                        // We're not in a string, this opens one
                        string_quote = Some(ch);
                    }
                }
                '{' if string_quote.is_none() => depth += 1,
                '}' if string_quote.is_none() => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }

        if end == 0 {
            return Err("Unterminated dict".to_string());
        }

        let dict_str = &s[..end];
        let dict = parse_python_dict(dict_str)?;
        Ok((PyValue::Dict(dict), &s[end..]))
    } else if s.starts_with('\'') || s.starts_with('"') {
        // Parse string
        let quote_char = s.chars().next().unwrap();
        let mut end = 1;
        let mut escape_next = false;

        for (i, ch) in s[1..].chars().enumerate() {
            if escape_next {
                escape_next = false;
                end = i + 2;
                continue;
            }

            if ch == '\\' {
                escape_next = true;
            } else if ch == quote_char {
                end = i + 2;
                break;
            }
            end = i + 2;
        }

        let value = s[1..end - 1].to_string();
        Ok((PyValue::String(value), &s[end..]))
    } else if let Some(stripped) = s.strip_prefix("None") {
        Ok((PyValue::None, stripped))
    } else {
        Err("Unknown value type".to_string())
    }
}

fn fuzzy_compare_sql(actual: &str, expected: &str) -> bool {
    let actual = eliminate_comments(actual);
    let expected = eliminate_comments(expected);
    let actual_abstract_tokens = abstract_tokenize(tokenize(&actual));
    let expected_abstract_tokens = abstract_tokenize(tokenize(&expected));

    let mut actual_index = 0;
    let mut expected_index = 0;
    let mut actual_abstract_token = None;
    let mut expected_abstract_token = None;
    while actual_index < actual_abstract_tokens.len()
        && expected_index < expected_abstract_tokens.len()
    {
        if actual_abstract_token.is_none() {
            actual_abstract_token = actual_abstract_tokens.get(actual_index).cloned();
        }
        if expected_abstract_token.is_none() {
            expected_abstract_token = expected_abstract_tokens.get(expected_index).cloned();
        }

        match (
            actual_abstract_token.as_ref().unwrap(),
            expected_abstract_token.as_ref().unwrap(),
        ) {
            (AbstractToken::Token(actual_token), AbstractToken::Token(expected_token)) => {
                let actual_token_value = actual_token.value.clone();
                let expected_token_value = expected_token.value.clone();
                if actual_token_value == expected_token_value
                    || (actual_token_value.to_lowercase() == "with"
                        && expected_token_value.to_lowercase() == "with")
                {
                    actual_abstract_token = None;
                    expected_abstract_token = None;
                    actual_index += 1;
                    expected_index += 1;
                } else if actual_token_value.starts_with(&expected_token_value) {
                    actual_abstract_token = Some(AbstractToken::Token(Token {
                        value: actual_token_value[expected_token_value.len()..].to_string(),
                        maybe_hash: false,
                    }));
                    expected_abstract_token = None;
                    expected_index += 1;
                } else if expected_token_value.starts_with(&actual_token_value) {
                    expected_abstract_token = Some(AbstractToken::Token(Token {
                        value: expected_token_value[actual_token_value.len()..].to_string(),
                        maybe_hash: false,
                    }));
                    actual_abstract_token = None;
                    actual_index += 1;
                } else {
                    return false;
                }
            }
            (AbstractToken::Hash { prefix, hash }, AbstractToken::Token(expected_token)) => {
                // e.g.
                // not_null_int_incident_io__inci_a94c7199c374113430d951145e2f84e8"
                // vs
                // not_null_int_incident_io__incident_field_entries_listed_unique_id"

                // First find the first 30 characters in expected
                let mut expected_prefix = expected_token.value.clone();
                expected_index += 1;
                while expected_prefix.len() < 30 {
                    if let Some(AbstractToken::Token(expected_token)) =
                        expected_abstract_tokens.get(expected_index)
                    {
                        expected_prefix = expected_prefix + &expected_token.value;
                        expected_index += 1;
                    } else {
                        break;
                    }
                }
                if expected_prefix.starts_with(prefix)
                    || expected_prefix
                        .strip_prefix("dbt_utils_")
                        .map(|s| s.starts_with(prefix) || prefix.starts_with(s))
                        .unwrap_or(false)
                {
                } else {
                    return false;
                }
                // Second, continue consuming expected tokens until the md5 hash matches the hash
                while expected_index < expected_abstract_tokens.len() {
                    match expected_abstract_tokens.get(expected_index).unwrap() {
                        AbstractToken::Token(expected_token) => {
                            let mut matched = false;
                            for (i, c) in expected_token.value.chars().enumerate() {
                                expected_prefix.push(c);
                                let expected_prefix_md5 =
                                    format!("{:x}", md5::compute(&expected_prefix));

                                if expected_prefix_md5 == *hash {
                                    matched = true;
                                    let expected_left_over =
                                        expected_token.value[i + 1..].to_string();
                                    if expected_left_over.is_empty() {
                                        expected_abstract_token = None;
                                        expected_index += 1;
                                    } else {
                                        expected_abstract_token =
                                            Some(AbstractToken::Token(Token {
                                                value: expected_left_over,
                                                maybe_hash: false,
                                            }));
                                    }
                                    break;
                                }
                            }
                            if !matched {
                                expected_index += 1;
                            } else {
                                break;
                            }
                        }
                        _ => {
                            return false;
                        }
                    }
                }
                actual_abstract_token = None;
                actual_index += 1;
            }
            (AbstractToken::Token(_), AbstractToken::Hash { .. }) => {
                return false;
            }
            (
                AbstractToken::Hash {
                    hash: actual_hash, ..
                },
                AbstractToken::Hash {
                    hash: expected_hash,
                    ..
                },
            ) => {
                // e.g.
                // source_unique_combination_of_c_7d86b29e62ff0d9a2521eecdb583ae14
                // vs
                // dbt_utils_source_unique_combin_7d86b29e62ff0d9a2521eecdb583ae14
                if actual_hash != expected_hash {
                    return false;
                }
                actual_abstract_token = None;
                expected_abstract_token = None;
                actual_index += 1;
                expected_index += 1;
            }
            // we don't care about the timestamp value
            (AbstractToken::Timestamp { .. }, AbstractToken::Timestamp { .. }) => {
                actual_abstract_token = None;
                expected_abstract_token = None;
                actual_index += 1;
                expected_index += 1;
            }
            (AbstractToken::Timestamp { .. }, _) | (_, AbstractToken::Timestamp { .. }) => {
                return false;
            }
        }
    }

    if actual_index == actual_abstract_tokens.len()
        && expected_index == expected_abstract_tokens.len()
    {
        return true;
    }

    false
}

/// Strip `/* ... */` block comments, respecting string literals. Leave `--` line comments
/// intact so `--EPHEMERAL-SELECT-WRAPPER-*` markers reach `abstract_tokenize`.
fn eliminate_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;

    while i < bytes.len() {
        let ch = bytes[i] as char;

        // Inside string literals — pass through verbatim, watch for closing quote.
        if in_single {
            out.push(ch);
            if ch == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            out.push(ch);
            if ch == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_backtick {
            out.push(ch);
            if ch == '`' {
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        // Open string literal.
        if ch == '\'' {
            in_single = true;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' {
            in_double = true;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '`' {
            in_backtick = true;
            out.push(ch);
            i += 1;
            continue;
        }

        // Block comment: skip to closing `*/`.
        if ch == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '*' {
            i += 2;
            while i + 1 < bytes.len() {
                if bytes[i] as char == '*' && bytes[i + 1] as char == '/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Line comment: preserve EPHEMERAL markers; skip everything else to EOL.
        if ch == '-' && i + 1 < bytes.len() && bytes[i + 1] as char == '-' {
            const EPHEMERAL: &str = "--EPHEMERAL-SELECT-WRAPPER";
            if bytes[i..].starts_with(EPHEMERAL.as_bytes()) {
                // Emit up to (but not including) the newline.
                while i < bytes.len() && bytes[i] as char != '\n' {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            } else {
                // Skip to end of line.
                while i < bytes.len() && bytes[i] as char != '\n' {
                    i += 1;
                }
            }
            continue;
        }

        out.push(ch);
        i += 1;
    }
    out
}

fn generate_visual_sql_diff(actual: &str, expected: &str) -> String {
    let mut diff_output = String::new();
    diff_output.push_str("Visual SQL Diff (ignoring all whitespace):\n");
    diff_output.push_str("==========================================\n\n");

    // Create normalized strings
    let actual_normalized = actual
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let expected_normalized = expected
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();

    // Compare normalized strings
    if actual_normalized == expected_normalized {
        diff_output.push_str("No differences found.\n");
        return diff_output;
    }

    // Show original SQL first
    diff_output.push_str("Original SQL:\n");
    diff_output.push_str("-------------\n");
    diff_output.push_str("Actual:\n");
    diff_output.push_str(&format!("{actual}\n\n"));
    diff_output.push_str("Expected:\n");
    diff_output.push_str(&format!("{expected}\n\n"));

    diff_output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_sql_identical_ignore_whitespace() {
        let sql1 = "SELECT   *\nFROM    users";
        let sql2 = "SELECT*FROMusers";

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should be OK when SQL is identical ignoring whitespace"
        );
    }

    #[test]
    fn test_compare_sql_case_sensitive() {
        let sql1 = "SELECT * FROM users";
        let sql2 = "select * from users";

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(result.is_err(), "Should fail when case differs");
    }

    #[test]
    fn test_compare_sql_different_content() {
        let sql1 = "SELECT * FROM users WHERE id = 1";
        let sql2 = "SELECT * FROM orders WHERE id = 2";

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(result.is_err(), "Should fail when SQL content differs");
    }

    #[test]
    fn test_compare_sql_length_difference() {
        let sql1 = "SELECT * FROM users";
        let sql2 = "SELECT * FROM users WHERE active = true";

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(result.is_err(), "Should fail when SQL length differs");
    }

    #[test]
    fn test_visual_diff_markers() {
        let sql1 = "SELECT id, name FROM users";
        let sql2 = "SELECT id, email FROM users";

        let diff = generate_visual_sql_diff(sql1, sql2);

        // Test that it shows both original and normalized versions
        assert!(diff.contains(sql1));
        assert!(diff.contains(sql2));
    }

    #[test]
    fn test_multiline_sql_ignores_newlines() {
        let sql1 = "SELECT\nu.id,\nu.name\nFROM users u";
        let sql2 = "SELECT u.id, u.name FROM users u";

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore newlines and whitespace differences"
        );
    }

    #[test]
    fn test_multiline_sql_detects_content_differences() {
        let sql1 = r#"SELECT
            u.id,
            u.name
        FROM users u"#;

        let sql2 = r#"SELECT
            u.id,
            u.email
        FROM users u"#;

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "Should detect content differences even with newlines"
        );
    }

    #[test]
    fn test_create_view_as_tolerates_asymmetric_wrapping_parens() {
        // `AS (<subquery>)` and `AS <subquery>` are always semantically identical. Some code
        // paths (e.g. dbt-core's hand-rolled `latest_version` pointer-view SQL) emit the bare
        // form while others always wrap the body in parens -- see fs#13705.
        let wrapped = "create or replace view db.sch.v as (\n    select * from db.sch.t\n  );";
        let bare = "create or replace view db.sch.v as select * from db.sch.t";

        compare_sql(wrapped, bare, AdapterType::Snowflake)
            .expect("wrapped vs bare CREATE VIEW body should compare as equal");
        compare_sql(bare, wrapped, AdapterType::Snowflake)
            .expect("bare vs wrapped CREATE VIEW body should compare as equal (order-independent)");
    }

    #[test]
    fn test_create_view_as_bare_form_still_detects_real_mismatches() {
        // Guard against over-relaxing: two bare (unwrapped) bodies that are actually different
        // must still be reported as a mismatch.
        let actual = "create or replace view db.sch.v as select * from db.sch.t1";
        let expected = "create or replace view db.sch.v as select * from db.sch.t2";

        assert!(
            compare_sql(actual, expected, AdapterType::Snowflake).is_err(),
            "genuinely different bare CREATE VIEW bodies must not compare as equal"
        );
    }

    #[test]
    fn test_bigquery_struct_field_order_drift_should_be_ignorable() {
        // Minimal repro for replay SQL mismatch when a query contains:
        //   to_json_string(struct(...))
        // and the STRUCT field order differs between recording (Mantle/dbt-core) and Fusion.
        let sql_recorded = r#"
create or replace table `db`.`sch`.`opportunity_product_entity_stream_base` as (
  with hashed_rows as (
    select
      to_hex(md5(to_json_string(
        struct( -- noqa: PRS
          b , a , c
        )
      ))) as row_hash_id
    from `db`.`sch`.`t`
  )
  select * from hashed_rows
);
"#;

        let sql_fusion = r#"
create or replace table `db`.`sch`.`opportunity_product_entity_stream_base` as (
  with hashed_rows as (
    select
      to_hex(md5(to_json_string(
        struct( -- noqa: PRS
          a , b , c
        )
      ))) as row_hash_id
    from `db`.`sch`.`t`
  )
  select * from hashed_rows
);
"#;

        compare_sql(sql_fusion, sql_recorded, AdapterType::Snowflake)
            .expect("STRUCT field order drift should be ignored");
    }

    #[test]
    fn test_bigquery_struct_projection_order_drift_should_be_ignorable() {
        // Same as the Snowplow ordering drift case, but without comments.
        let expected_alias_order = [
            "service_configuration_context",
            "modal_entity",
            "experiment_entity",
            "kafka_connector_entity",
            "selected_item_context",
            "service_integration_context",
            "workflow_entity",
            "video_entity",
            "value_calculator_context",
            "kafka_plan_finder_context",
            "feature_flag_context",
            "posthog_session_context",
            "pardot_context",
        ];

        let actual_alias_order = [
            "experiment_entity",
            "feature_flag_context",
            "kafka_connector_entity",
            "kafka_plan_finder_context",
            "modal_entity",
            "pardot_context",
            "posthog_session_context",
            "selected_item_context",
            "service_configuration_context",
            "service_integration_context",
            "value_calculator_context",
            "video_entity",
            "workflow_entity",
        ];

        let projection_for = |alias: &str| -> String { format!("STRUCT(1 AS x) AS {alias}") };

        let expected_sql = format!(
            "select {} from t",
            expected_alias_order
                .iter()
                .map(|a| projection_for(a))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let actual_sql = format!(
            "select {} from t",
            actual_alias_order
                .iter()
                .map(|a| projection_for(a))
                .collect::<Vec<_>>()
                .join(", ")
        );

        compare_sql(&actual_sql, &expected_sql, AdapterType::Bigquery)
            .expect("projection order drift should be ignored");
    }

    #[test]
    fn test_bigquery_struct_projection_order_drift_with_comment_apostrophe_should_be_ignorable() {
        // Minimal repro for the *ordering-only* Snowplow SQL mismatch observed in replay:
        //
        // Some Jinja/adapter code paths build a list of `STRUCT(...) AS <context_alias>` projections
        // by iterating a map. Different runners/recorders may emit identical projections in
        // different orders (e.g. insertion-order vs key-sorted iteration). We treat this drift as
        // ignorable for replay via canonicalization.
        //
        // This variant includes a `--` comment containing an apostrophe (e.g. "hasn't"), which
        // appears in the real project SQL. Replay should still treat projection ordering drift as
        // ignorable.
        let expected_alias_order = [
            "service_configuration_context",
            "modal_entity",
            "experiment_entity",
            "kafka_connector_entity",
            "selected_item_context",
            "service_integration_context",
            "workflow_entity",
            "video_entity",
            "value_calculator_context",
            "kafka_plan_finder_context",
            "feature_flag_context",
            "posthog_session_context",
            "pardot_context",
        ];

        let actual_alias_order = [
            "experiment_entity",
            "feature_flag_context",
            "kafka_connector_entity",
            "kafka_plan_finder_context",
            "modal_entity",
            "pardot_context",
            "posthog_session_context",
            "selected_item_context",
            "service_configuration_context",
            "service_integration_context",
            "value_calculator_context",
            "video_entity",
            "workflow_entity",
        ];

        let projection_for = |alias: &str| -> String {
            // Include a comment with an apostrophe on one item to mimic the real model SQL.
            if alias == "selected_item_context" {
                format!("STRUCT(1 AS x) AS {alias} -- hasn't been canonicalized yet\n")
            } else {
                format!("STRUCT(1 AS x) AS {alias}")
            }
        };

        let expected_sql = format!(
            "select {} from t",
            expected_alias_order
                .iter()
                .map(|a| projection_for(a))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let actual_sql = format!(
            "select {} from t",
            actual_alias_order
                .iter()
                .map(|a| projection_for(a))
                .collect::<Vec<_>>()
                .join(", ")
        );

        compare_sql(&actual_sql, &expected_sql, AdapterType::Bigquery)
            .expect("projection order drift should be ignored even with apostrophes in comments");
    }

    #[test]
    fn test_bigquery_simple_projection_order_drift_with_comment_apostrophe_should_be_ignorable() {
        // Minimal repro for the next Snowplow ordering drift surface:
        //
        // Downstream models may select the already-built context columns as plain identifiers:
        //   , experiment_entity
        //   , feature_flag_context
        //   , ...
        //
        // Mantle vs Fusion can emit these in different orders due to map iteration. This should be
        // ignorable for replay, even when the SELECT list contains `--` comments with apostrophes.
        //
        // This test is expected to FAIL (SqlMismatch) until we canonicalize identifier projection
        // ordering drift for this pattern.
        let expected_order = [
            "service_configuration_context",
            "modal_entity",
            "experiment_entity",
            "kafka_connector_entity",
            "selected_item_context",
            "service_integration_context",
            "workflow_entity",
            "video_entity",
            "value_calculator_context",
            "kafka_plan_finder_context",
            "feature_flag_context",
            "posthog_session_context",
            "pardot_context",
        ];
        let actual_order = [
            "experiment_entity",
            "feature_flag_context",
            "kafka_connector_entity",
            "kafka_plan_finder_context",
            "modal_entity",
            "pardot_context",
            "posthog_session_context",
            "selected_item_context",
            "service_configuration_context",
            "service_integration_context",
            "value_calculator_context",
            "video_entity",
            "workflow_entity",
        ];

        let projection_for = |alias: &str| -> String {
            if alias == "selected_item_context" {
                format!("{alias} -- hasn't been canonicalized yet\n")
            } else {
                alias.to_string()
            }
        };

        let expected_sql = format!(
            "select event_id, {} from t",
            expected_order
                .iter()
                .map(|a| projection_for(a))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let actual_sql = format!(
            "select event_id, {} from t",
            actual_order
                .iter()
                .map(|a| projection_for(a))
                .collect::<Vec<_>>()
                .join(", ")
        );

        compare_sql(&actual_sql, &expected_sql, AdapterType::Bigquery).expect(
            "simple projection order drift should be ignored even with apostrophes in comments",
        );
    }

    #[test]
    fn test_bigquery_forward_fill_column_order_drift_should_be_ignorable() {
        // Minimal repro for replay SQL mismatch when a query is generated from a set/list of
        // columns (order nondeterministic) and emitted as a SELECT projection list.
        let sql_recorded = r#"
with filled_data as (
  select 1 as billing_group_id, timestamp('2020-01-01') as __as_of, 1 as rn, 'x' as a, 'y' as b
)
select
  billing_group_id,
  __as_of,
  last_value(a ignore nulls) over (
    partition by billing_group_id
    order by __as_of
    rows between unbounded preceding and current row
  ) as a,
  last_value(b ignore nulls) over (
    partition by billing_group_id
    order by __as_of
    rows between unbounded preceding and current row
  ) as b,
  rn
from filled_data
qualify row_number() over (partition by billing_group_id, __as_of order by rn desc) = 1
"#;

        let sql_fusion = r#"
with filled_data as (
  select 1 as billing_group_id, timestamp('2020-01-01') as __as_of, 1 as rn, 'x' as a, 'y' as b
)
select
  billing_group_id,
  __as_of,
  last_value(b ignore nulls) over (
    partition by billing_group_id
    order by __as_of
    rows between unbounded preceding and current row
  ) as b,
  last_value(a ignore nulls) over (
    partition by billing_group_id
    order by __as_of
    rows between unbounded preceding and current row
  ) as a,
  rn
from filled_data
qualify row_number() over (partition by billing_group_id, __as_of order by rn desc) = 1
"#;

        compare_sql(sql_fusion, sql_recorded, AdapterType::Snowflake)
            .expect("Forward-fill projection column order drift should be ignored");
    }

    #[test]
    fn test_split_union_top_level_splits_and_handles_unicode() {
        // Regression test: previously this could panic if the scan index landed in the middle
        // of a multi-byte UTF-8 char (e.g. “).
        let sql = "select 1 as a /* “unicode” */ UNION      select 2 as b";
        let parts = split_union_top_level(sql).expect("should split on top-level UNION");
        assert_eq!(parts, vec!["select 1 as a", "select 2 as b"]);
    }

    #[test]
    fn test_split_union_all_top_level_splits_and_handles_unicode() {
        // Regression test: previously this could panic if the scan index landed in the middle
        // of a multi-byte UTF-8 char (e.g. “).
        let sql = "select 1 as a /* “unicode” */ UNION   ALL   select 2 as b";
        let parts = split_union_all_top_level(sql).expect("should split on top-level UNION ALL");
        assert_eq!(parts, vec!["select 1 as a", "select 2 as b"]);
    }

    #[test]
    fn test_split_union_all_top_level_does_not_split_inside_parentheses() {
        let sql = "select 1 as a union all select (select 2 as b union all select 3 as c)";
        let parts =
            split_union_all_top_level(sql).expect("should split on the top-level UNION ALL");
        assert_eq!(
            parts,
            vec![
                "select 1 as a",
                "select (select 2 as b union all select 3 as c)"
            ]
        );
    }

    #[test]
    fn test_empty_sql_comparison() {
        let result1 = compare_sql("", "", AdapterType::Snowflake);
        assert!(result1.is_ok(), "Empty SQL should match empty SQL");

        let result2 = compare_sql("SELECT 1", "", AdapterType::Snowflake);
        assert!(result2.is_err(), "Non-empty SQL should not match empty SQL");

        let result3 = compare_sql("", "SELECT 1", AdapterType::Snowflake);
        assert!(result3.is_err(), "Empty SQL should not match non-empty SQL");
    }

    #[test]
    fn test_whitespace_only_sql() {
        let sql1 = "   \n\t  ";
        let sql2 = "  \t\n   ";

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Whitespace-only SQL should match regardless of type/order"
        );
    }

    #[test]
    fn test_placeholder_replacement_differences() {
        let actual = "SELECT %s, %s FROM table";
        let expected = "SELECT 1, 'test' FROM table";

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "Should detect placeholder vs value differences"
        );
    }

    #[test]
    fn test_databricks_column_comment_legacy_and_modern_syntax_equivalent() {
        // Databricks supports persisting column comments via either:
        // - legacy: ALTER TABLE .. CHANGE COLUMN .. COMMENT '...'
        // - modern: COMMENT ON COLUMN .. IS '...'
        //
        // These are semantically equivalent and should not cause replay-mode SQL mismatches.
        let actual = "alter table `dbt`.`dbt_entities`.`ent_shopify_inventory_quantity` change column id comment 'Primary key for the inventory quantity record.';";
        let expected = "COMMENT ON COLUMN `dbt`.`dbt_entities`.`ent_shopify_inventory_quantity`.`id` IS 'Primary key for the inventory quantity record.'";

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Expected legacy ALTER TABLE CHANGE COLUMN COMMENT to be equivalent to COMMENT ON COLUMN"
        );
    }

    #[test]
    fn test_snowflake_grant_select_python_list_equivalent_to_multiple_statements() {
        let actual = r#"
            grant select on SILVER_DEV.PRODUCT.products to ['ANALYTICS_PRODUCT', 'ANALYTICS', 'DATA_ENGINEERING'];
        "#;
        let expected = r#"
            grant select on SILVER_DEV.PRODUCT.products to DATA_ENGINEERING;
            grant select on SILVER_DEV.PRODUCT.products to ANALYTICS;
            grant select on SILVER_DEV.PRODUCT.products to ANALYTICS_PRODUCT;
        "#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Expected python-list GRANT form to be equivalent to multiple GRANT statements"
        );
    }

    #[test]
    fn test_snowflake_revoke_privilege_order_drift_should_be_ignorable() {
        let actual = r#"
            revoke delete on DB.SCH.tbl from ROLE_A;
            revoke rebuild on DB.SCH.tbl from ROLE_A;
            revoke evolve schema on DB.SCH.tbl from ROLE_A;
            revoke select error table on DB.SCH.tbl from ROLE_A;
            revoke truncate on DB.SCH.tbl from ROLE_A;
            revoke update on DB.SCH.tbl from ROLE_A;
            revoke insert on DB.SCH.tbl from ROLE_A;
            revoke references on DB.SCH.tbl from ROLE_A;
            revoke select on DB.SCH.tbl from ROLE_A;
            revoke applybudget on DB.SCH.tbl from ROLE_A;
            grant all on DB.SCH.tbl to ROLE_A;
        "#;
        let expected = r#"
            revoke select error table on DB.SCH.tbl from ROLE_A;
            revoke delete on DB.SCH.tbl from ROLE_A;
            revoke rebuild on DB.SCH.tbl from ROLE_A;
            revoke evolve schema on DB.SCH.tbl from ROLE_A;
            revoke insert on DB.SCH.tbl from ROLE_A;
            revoke truncate on DB.SCH.tbl from ROLE_A;
            revoke update on DB.SCH.tbl from ROLE_A;
            revoke references on DB.SCH.tbl from ROLE_A;
            revoke select on DB.SCH.tbl from ROLE_A;
            revoke applybudget on DB.SCH.tbl from ROLE_A;
            grant all on DB.SCH.tbl to ROLE_A;
        "#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Expected Snowflake revoke privilege ordering drift to be ignored"
        );
    }

    #[test]
    fn test_snowflake_grant_revoke_group_order_is_not_ignorable() {
        let actual = r#"
            revoke select on DB.SCH.tbl from ROLE_A;
            grant all on DB.SCH.tbl to ROLE_A;
        "#;
        let expected = r#"
            grant all on DB.SCH.tbl to ROLE_A;
            revoke select on DB.SCH.tbl from ROLE_A;
        "#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "Expected grant/revoke group ordering to remain significant"
        );
    }

    #[test]
    fn test_complex_whitespace_scenarios() {
        // Test various whitespace combinations
        let scenarios = vec![
            ("SELECT\t*\nFROM\r\ntable", "SELECT * FROM table"),
            ("  SELECT  *  FROM  table  ", "SELECT*FROMtable"),
            ("SELECT\n\n\n*\n\n\nFROM\n\n\ntable", "SELECT * FROM table"),
        ];

        for (sql1, sql2) in scenarios {
            let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
            assert!(
                result.is_ok(),
                "Should ignore all whitespace variations: '{sql1}' vs '{sql2}'"
            );
        }
    }

    #[test]
    fn test_trailing_semicolon_is_ignorable_for_single_statement() {
        // Semicolons are statement terminators and should be treated as ignorable for replay
        // SQL matching when the statements are otherwise identical.
        let actual = r#"
            SELECT column_name, tag_name, tag_value
              FROM `system`.`information_schema`.`column_tags`
             WHERE catalog_name = 'dbt'
               AND schema_name = 'dbt_staging'
               AND table_name = 'stg_shopify_order_stage'
        "#;

        let expected = r#"
            SELECT column_name, tag_name, tag_value
              FROM `system`.`information_schema`.`column_tags`
             WHERE catalog_name = 'dbt'
               AND schema_name = 'dbt_staging'
               AND table_name = 'stg_shopify_order_stage';
        "#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Trailing semicolon should not cause SQL mismatch for single-statement queries"
        );
    }

    #[test]
    fn test_case_sensitivity_preserved() {
        // These should be different because case matters
        let test_cases = vec![
            ("SELECT", "select"),
            ("FROM", "from"),
            ("WHERE", "where"),
            ("users", "USERS"),
        ];

        for (upper, lower) in test_cases {
            let sql1 = format!("{upper} * FROM table");
            let sql2 = format!("{lower} * FROM table");

            let result = compare_sql(&sql1, &sql2, AdapterType::Snowflake);
            assert!(
                result.is_err(),
                "Should be case sensitive: '{upper}' vs '{lower}'"
            );
        }
    }

    #[test]
    fn test_empty_and_whitespace_edge_cases() {
        let test_cases = vec![
            ("", "", true),           // Both empty should match
            ("   ", "\t\n", true),    // All whitespace should match
            ("SELECT", "", false),    // Content vs empty should not match
            ("", "SELECT", false),    // Empty vs content should not match
            ("   ", "SELECT", false), // Whitespace vs content should not match
        ];

        for (sql1, sql2, should_match) in test_cases {
            let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
            if should_match {
                assert!(result.is_ok(), "Should match: '{sql1}' vs '{sql2}'");
            } else {
                assert!(result.is_err(), "Should not match: '{sql1}' vs '{sql2}'");
            }
        }
    }

    #[test]
    fn test_with_clause_vs_simple_select() {
        let simple_select = "SELECT * FROM users";
        let with_clause_select = r#"WITH temp_table AS (
            SELECT id, name FROM customers
        )
        SELECT * FROM users"#;

        let result = compare_sql(simple_select, with_clause_select, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "Should detect difference between simple SELECT and WITH clause"
        );
    }

    #[test]
    fn test_compare_sql_with_truncated_test_name() {
        let sql1 = r#"    alter session set query_tag = '{"dbt_environment_name": "default", "dbt_job_id": "not set", "dbt_run_id": "not set", "dbt_run_reason": "development_and_testing", "dbt_project_name": "fishtown_internal_analytics", "dbt_user_name": "ZHONG.XU", "dbt_model_name": "not_null_int_incident_io__inci_a94c7199c374113430d951145e2f84e8", "dbt_materialization_type": "test", "dbt_incremental_full_refresh": "false", "dbt_is_cold_storage_refresh": "false", "dbt_invocation_env": "null"}'"#;
        let sql2 = r#"    alter session set query_tag = '{"dbt_environment_name": "default", "dbt_job_id": "not set", "dbt_run_id": "not set", "dbt_run_reason": "development_and_testing", "dbt_project_name": "fishtown_internal_analytics", "dbt_user_name": "ZHONG.XU", "dbt_model_name": "not_null_int_incident_io__incident_field_entries_listed_unique_id", "dbt_materialization_type": "test", "dbt_incremental_full_refresh": "false", "dbt_is_cold_storage_refresh": "false", "dbt_invocation_env": "null"}'"#;

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore difference between truncated and full test name"
        );
    }

    #[test]
    fn test_compare_sql_with_dbt_utils_table_name() {
        let sql1 = r#"create or replace transient table analytics_dev.dbt_zhongxu.source_unique_combination_of_c_7d86b29e62ff0d9a2521eecdb583ae14
             as
            (
    with validation_errors as (
        select
            incident_id, incident_timestamp_id
        from raw.fivetran_incidentio.incident_timestamp_value
        group by incident_id, incident_timestamp_id
        having count(*) > 1
    )
    select *
    from validation_errors
            );"#;
        let sql2 = r#"create or replace transient table analytics_dev.dbt_zhongxu.dbt_utils_source_unique_combin_7d86b29e62ff0d9a2521eecdb583ae14
        as (
    with validation_errors as (
        select
            incident_id, incident_timestamp_id
        from raw.fivetran_incidentio.incident_timestamp_value
        group by incident_id, incident_timestamp_id
        having count(*) > 1
    )
    select *
    from validation_errors
        )
    ;
    "#;

        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(result.is_ok(), "Should ignore difference for dbt_utils_");
    }

    #[test]
    fn test_compare_sql_with_dbt_utils_table_name_2() {
        let sql1 = r#"alter session set query_tag = '{"dbt_environment_name": "default", "dbt_job_id": "not set", "dbt_run_id": "not set", "dbt_run_reason": "development_and_testing", "dbt_project_name": "fishtown_internal_analytics", "dbt_user_name": "ZHONG.XU", "dbt_model_name": "source_unique_combination_of_c_7d86b29e62ff0d9a2521eecdb583ae14", "dbt_materialization_type": "test", "dbt_incremental_full_refresh": "false", "dbt_is_cold_storage_refresh": "false", "dbt_invocation_env": "null"}'"#;
        let sql2 = r#"alter session set query_tag = '{"dbt_environment_name": "default", "dbt_job_id": "not set", "dbt_run_id": "not set", "dbt_run_reason": "development_and_testing", "dbt_project_name": "fishtown_internal_analytics", "dbt_user_name": "ZHONG.XU", "dbt_model_name": "dbt_utils_source_unique_combination_of_columns_incident_io_incident_timestamp_value_incident_id__incident_timestamp_id", "dbt_materialization_type": "test", "dbt_incremental_full_refresh": "false", "dbt_is_cold_storage_refresh": "false", "dbt_invocation_env": "null"}'"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(result.is_ok(), "Should ignore difference for dbt_utils_");
    }

    #[test]
    fn test_compare_sql_timestamp() {
        let sql1 = r#"delete from ANALYTICS.intermediate.int_serp_trends
      where created_date >= '2025-09-10T18:07:45.449898-07:00'"#;
        let sql2 = r#"delete from ANALYTICS.intermediate.int_serp_trends
      where created_date >= '2025-09-10T14:16:52.500487'"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore difference for timestamp value difference"
        );

        // Additional timestamp drift case (incomplete SQL is fine; we only compare text).
        let sql3 = r#"
with cur as (

    with baseline as (
        select lower(column_name) as column_name, data_type
        from PROD_ASKO_SERVERING.elementary.test_ALPHA__tmp_TIMESTAMP
    )

    select
        columns_snapshot.full_table_name,
        lower(columns_snapshot.column_name) as column_name,
        columns_snapshot.data_type,
        (baseline.column_name IS NULL) as is_new,

    cast ('2025-12-30T06:50:06' as timestamp)
"#;

        let sql4 = r#"
with cur as (

    with baseline as (
        select lower(column_name) as column_name, data_type
        from PROD_ASKO_SERVERING.elementary.test_ALPHA__tmp_TIMESTAMP
    )

    select
        columns_snapshot.full_table_name,
        lower(columns_snapshot.column_name) as column_name,
        columns_snapshot.data_type,
        (baseline.column_name IS NULL) as is_new,

    cast ('2025-12-23T07:06:04' as timestamp)
"#;

        let result = compare_sql(sql3, sql4, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore difference for timestamp value difference inside CTE fragment"
        );

        // Timestamp literal drift with timezone: space separator vs T separator.
        let sql5 = r#"
        select
            min(bucket_start) as min_bucket_start,
            cast('2025-12-31T08:17:34+00:00' as timestamp) as max_bucket_end
"#;
        let sql6 = r#"
        select
            min(bucket_start) as min_bucket_start,
            cast('2025-12-23 08:28:37+00:00' as timestamp) as max_bucket_end
"#;
        let result = compare_sql(sql5, sql6, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore differences for timestamp value drift in cast() literal with timezone"
        );
    }

    #[test]
    fn test_compare_sql_with_test_name_variation() {
        // invocation_id + test unique_id embedded inside string literal.
        let sql1 = r#"
  md5(cast(coalesce(cast(data_issue_id as varchar), '') || '-' || coalesce(cast(cast('019b6e05-5c15-7831-8c40-6718c8683411.test.dis_asko_servering.elementary_schema_changes_from_370c8b8ac782c433a20c7ab43b202251.fba8a27235' as varchar) as varchar), '') as TEXT)) as id,
"#;
        let sql2 = r#"
  md5(cast(coalesce(cast(data_issue_id as varchar), '') || '-' || coalesce(cast(cast('c406f8ee-28dd-4a60-91eb-639ae6a8a613.test.dis_asko_servering.elementary_schema_changes_from_baseline_prs_dim_hendelse_innholdsnavn_.c99b82db3f' as varchar) as varchar), '') as TEXT)) as id,
"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore differences for invocation_id/test unique_id embedded in string literal"
        );

        // test unique_id literal (no UUID prefix).
        let sql3 = r#"
        cast('test.dis_asko_servering.elementary_schema_changes_from_ebfd1280ea747a1645e253f0e83e355e.83cf5105a0' as varchar) as test_unique_id,
"#;
        let sql4 = r#"
cast('test.dis_asko_servering.elementary_schema_changes_from_baseline_prs_dim_hendelse_hendelseskategori_.4d86bc1ad2' as varchar) as test_unique_id,
"#;
        let result = compare_sql(sql3, sql4, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore differences for test unique_id embedded in string literal"
        );

        // Quoted vs unquoted test temp relation identifier in DDL.
        let sql5 = r#"create or replace  table BE_DPL_PR.elementary.test_ALPHA__tmp_TIMESTAMP"#;
        let sql6 = r#"create or replace table BE_DPL_PR.elementary."test_ALPHA__tmp_TIMESTAMP""#;
        let result = compare_sql(sql5, sql6, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore differences for quoted vs unquoted test temp relation identifier"
        );
    }

    #[test]
    fn test_compare_sql_timestamp_ignore_t() {
        let sql1 = r#"delete from ANALYTICS.intermediate.int_serp_trends
      where created_date >= '2025-09-10T18:07:45.449898'"#;
        let sql2 = r#"delete from ANALYTICS.intermediate.int_serp_trends
      where created_date >= '2025-09-1014:16:52.500487'"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore difference for timestamp value difference"
        );
    }

    #[test]
    fn test_compare_sql_timestamp_ignore_t2() {
        let sql1 = r#"delete from ANALYTICS.intermediate.int_serp_trends
      where created_date >= '2025-09-10T18:07:45.449898'"#;
        let sql2 = r#"delete from ANALYTICS.intermediate.int_serp_trends
      where created_date >= '2025-09-10 14:16:52.500487'"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should ignore difference for timestamp value difference"
        );
    }

    #[test]
    fn test_compare_sql_timestamp_in_cast_with_space_separator() {
        let sql1 = "select cast ('2025-12-23 07:06:03' as timestamp)";
        let sql2 = "select cast ('2025-12-30 06:11:01' as timestamp)";
        compare_sql(sql1, sql2, AdapterType::Snowflake).unwrap_or_else(|e| {
            panic!(
                "Should ignore difference for timestamp value difference in cast() literal, but got:\n{e}"
            )
        });
    }

    #[test]
    fn test_canonicalize_quoted_timestamp_space_separator_values() {
        let sql1 = "cast ('2025-12-23 07:06:03' as timestamp)";
        let sql2 = "cast ('2025-12-30 06:11:01' as timestamp)";

        let out1 = canonicalize_quoted_timestamp_space_separator(sql1);
        let out2 = canonicalize_quoted_timestamp_space_separator(sql2);

        assert!(
            out1.contains("'2025-12-23T07:06:03'"),
            "Expected canonicalized timestamp literal, got: {out1}"
        );
        assert!(
            out2.contains("'2025-12-30T06:11:01'"),
            "Expected canonicalized timestamp literal, got: {out2}"
        );
    }

    #[test]
    fn test_compare_ephemeral_model() {
        let sql1 = r#"
create or replace transient table x.y.z
    as (with u as (
with
v as (
    select 1
from w
),
select *
from unioned
)
--EPHEMERAL-SELECT-WRAPPER-START
select * from (
with base as (
    select *
    from u
)
select *
from aggregated
--EPHEMERAL-SELECT-WRAPPER-END
)
    )
;
"#;
        let sql2 = r#"
create or replace transient table x.y.z
    as (with u as (
with
v as (
    select 1
from w
),
select *
from unioned
)
, base as (
    select *
    from u
)
select *
from aggregated
    )
;"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(result.is_ok(), "Should match");
    }

    #[test]
    fn test_comment_in_ephemeral_model() {
        let sql1 = r#"
create or replace  temporary view DB.SCHEMA.model_name__dbt_tmp

   as (
    with __dbt__cte__stg_source_a as (
SELECT
  *
FROM
  source_db.metadata.table_a
), __dbt__cte__stg_source_b as (
SELECT
  *
FROM
  source_db.metadata.table_b
)
--EPHEMERAL-SELECT-WRAPPER-START
select * from (


-- Do not allow a full refresh of this model




-- This model contains aggregated statistics
-- Every day, the data is extracted and stored for analysis

WITH aggregated_data AS (
  SELECT
    entity_id
    , COUNT(DISTINCT field_name) as field_count
  FROM
    __dbt__cte__stg_source_a
  GROUP BY entity_id
)

SELECT
  t.schema_name
  , t.entity_name
  , t.num_rows
  , t.size_bytes
  , s.field_count
  , CURRENT_DATE() AS snapshot_date
FROM
  __dbt__cte__stg_source_b t
LEFT OUTER JOIN
  aggregated_data s ON t.entity_id = s.entity_id
--EPHEMERAL-SELECT-WRAPPER-END
)
  );
"#;
        let sql2 = r#"
create or replace  temporary view DB.SCHEMA.model_name__dbt_tmp




  as (


-- Do not allow a full refresh of this model




-- This model contains aggregated statistics
-- Every day, the data is extracted and stored for analysis

WITH  __dbt__cte__stg_source_a as (
SELECT
  *
FROM
  source_db.metadata.table_a
),  __dbt__cte__stg_source_b as (
SELECT
  *
FROM
  source_db.metadata.table_b
), aggregated_data AS (
  SELECT
    entity_id
    , COUNT(DISTINCT field_name) as field_count
  FROM
    __dbt__cte__stg_source_a
  GROUP BY entity_id
)

SELECT
  t.schema_name
  , t.entity_name
  , t.num_rows
  , t.size_bytes
  , s.field_count
  , CURRENT_DATE() AS snapshot_date
FROM
  __dbt__cte__stg_source_b t
LEFT OUTER JOIN
  aggregated_data s ON t.entity_id = s.entity_id
  );
"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(result.is_ok(), "Should match");
    }

    #[test]
    fn test_compare_ephemeral_model_with_block_comment() {
        let sql1 = r#"
create or replace view `db`.`schema`.`my_model`
OPTIONS()
as with __dbt__cte__base as (
SELECT id FROM `project`.`dataset`.`source_table`
)
--EPHEMERAL-SELECT-WRAPPER-START
select * from (
/* This model joins the base CTE with enrichment data. */
WITH enriched AS (
    SELECT * FROM __dbt__cte__base
)
SELECT * FROM enriched
--EPHEMERAL-SELECT-WRAPPER-END
);
"#;
        let sql2 = r#"
create or replace view `db`.`schema`.`my_model`
OPTIONS()
as /* This model joins the base CTE with enrichment data. */
WITH __dbt__cte__base as (
SELECT id FROM `project`.`dataset`.`source_table`
), enriched AS (
    SELECT * FROM __dbt__cte__base
)
SELECT * FROM enriched;
"#;
        let result = compare_sql(sql1, sql2, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should match: block comment in wrapper body"
        );
    }

    #[test]
    fn test_databricks_temp_view_vs_persisted_view_equivalent() {
        // Mantle recordings (dbt-databricks) may emit:
        //   create or replace temporary view `sources__dbt_tmp` as ...
        //
        // Fusion may emit (not temporary, and fully-qualified), plus extra line comments:
        //   -- DIVERGENCE
        //   create or replace view `dbt`.`dbt_dbt_audit`.`sources__dbt_tmp` as ...
        //
        // For replay, these should be treated as equivalent when the query body is identical and the
        // view name is a dbt tmp relation.
        let actual = r#"
-- DIVERGENCE
create or replace view `dbt`.`dbt_dbt_audit`.`sources__dbt_tmp` as
/* Bigquery won't let us `where` without `from` so we use this workaround */
with dummy_cte as (select 1 as foo)
select
    cast(null as string) as command_invocation_id
from dummy_cte
where 1 = 0
"#;

        let expected = r#"
create or replace temporary view `sources__dbt_tmp` as
/* Bigquery won't let us `where` without `from` so we use this workaround */
with dummy_cte as (select 1 as foo)
select
    cast(null as string) as command_invocation_id
from dummy_cte
where 1 = 0
"#;

        compare_sql(actual, expected, AdapterType::Databricks)
            .expect("should treat persisted view vs temp view as equivalent");

        // Same pattern but in a MERGE statement: Fusion uses a three-part qualified name for the
        // __dbt_tmp temp table in the USING clause, while Mantle uses just the identifier.
        let merge_actual = r#"
-- back compat for old kwarg name



    merge
    into
        `dbt`.`dbt_dbt_audit`.`seed_executions` as DBT_INTERNAL_DEST
    using
        `dbt`.`dbt_dbt_audit`.`seed_executions__dbt_tmp` as DBT_INTERNAL_SOURCE
    on
        FALSE
    when matched
        then update set
            *
    when not matched
        then insert
            *
"#;

        let merge_expected = r#"
-- back compat for old kwarg name






    merge
    into
        `dbt`.`dbt_dbt_audit`.`seed_executions` as DBT_INTERNAL_DEST
    using
        `seed_executions__dbt_tmp` as DBT_INTERNAL_SOURCE
    on
        FALSE
    when matched
        then update set
            *
    when not matched
        then insert
            *
"#;

        compare_sql(merge_actual, merge_expected, AdapterType::Databricks)
            .expect("should treat qualified vs unqualified __dbt_tmp in MERGE USING as equivalent");
    }

    #[test]
    fn test_compare_sql_query_tag_payload_ignored() {
        let actual = r#"    alter session set query_tag = '{""model_name"":""stg_base_orders"",""env"":""PRD"",""job"":{""run_id"":"""",""execution_date"":"""",""start_date"":""""}}'"#;
        let expected = r#"    alter session set query_tag = '{""env"": ""PRD"", ""job"": {""execution_date"": """", ""run_id"": """", ""start_date"": """"}, ""model_name"": ""stg_base_orders""}'"#;
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Query tag payload differences should be ignored"
        );
    }

    #[test]
    fn test_compare_sql_uuid_literals_ignored() {
        let actual = r#"
INSERT INTO
    PROD_SSAP_AUDIT.ABAC.ABAC_JOB_RUN
    (
        system_run_id,
        job_id,
        batch_run_id,
        job_start_dttm,
        job_end_dttm,
        job_start_dttm_utc,
        job_end_dttm_utc,
        job_status,
        last_updt_dttm,
        last_updt_uid
    )
SELECT
    '019a71ca-e5ad-7ca3-99d8-49b58a470d82' AS system_run_id,
    962 AS job_id,
    47217 AS batch_run_id,
    CURRENT_TIMESTAMP() AS job_start_dttm,
    NULL AS job_end_dttm,
    CONVERT_TIMEZONE('UTC', CURRENT_TIMESTAMP()) AS job_start_dttm_utc,
    NULL AS job_end_dttm_utc,
    'RUNNING' AS job_status,
    CURRENT_TIMESTAMP() AS last_updt_dttm,
    CURRENT_USER() AS last_updt_uid
FROM
    PROD_SSAP_AUDIT.ABAC.ABAC_JOB AS abac_job
WHERE
    abac_job.job_target = 'ldw_prtnr_all_wk_sumr_sales'
        "#;

        let expected = r#"
INSERT INTO
    PROD_SSAP_AUDIT.ABAC.ABAC_JOB_RUN
    (
        system_run_id,
        job_id,
        batch_run_id,
        job_start_dttm,
        job_end_dttm,
        job_start_dttm_utc,
        job_end_dttm_utc,
        job_status,
        last_updt_dttm,
        last_updt_uid
    )
SELECT
    '8f439b7e-752f-460a-8d1a-f469231d169c' AS system_run_id,
    962 AS job_id,
    47217 AS batch_run_id,
    CURRENT_TIMESTAMP() AS job_start_dttm,
    NULL AS job_end_dttm,
    CONVERT_TIMEZONE('UTC', CURRENT_TIMESTAMP()) AS job_start_dttm_utc,
    NULL AS job_end_dttm_utc,
    'RUNNING' AS job_status,
    CURRENT_TIMESTAMP() AS last_updt_dttm,
    CURRENT_USER() AS last_updt_uid
FROM
    PROD_SSAP_AUDIT.ABAC.ABAC_JOB AS abac_job
WHERE
    abac_job.job_target = 'ldw_prtnr_all_wk_sumr_sales'
        "#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(result.is_ok(), "UUID literal differences should be ignored");
    }

    #[test]
    fn test_compare_sql_whitespace_padded_uuid_literals_ignored() {
        // Regression for fs#14150: some customer macros pad the UUID literal with spaces
        // inside the quotes (e.g. an `invocation_id`-derived audit column), which the UUID
        // regex previously required to hug the quotes with no interior whitespace.
        let actual = r#"select * from (
    select ' 01a050c0-a25d-75d1-bc8d-e46fde65bb69 '::varchar as idp_dbt_id
) as __dbt_sbq where false limit 0"#;

        let expected = r#"select * from (
    select ' 66f899d8-7787-4998-8e66-ac219ca47493 '::varchar as idp_dbt_id
) as __dbt_sbq where false limit 0"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Whitespace-padded UUID literal differences should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_wrapper_diff_only_equivalence() {
        // Simple case: one side wraps the other in select * from ( ... )
        let with_cte = r#"
with base as (
    select 1 as id
)
select *
from base
"#;
        let wrapped = r#"
select * from (
with base as (
    select 1 as id
)
select *
from base
)
"#;
        let result = compare_sql(wrapped, with_cte, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Wrapper-only difference with identical body should be ignored"
        );
    }

    #[test]
    fn test_compare_sql_elementary_tmp_suffix_ignored() {
        let actual = r#"
create or replace temporary table abc_db.abc_production_models_elementary.dbt_sources__tmp_20251203160139043240
as (

    SELECT
        *
    FROM abc_db.abc_production_models_elementary.dbt_sources
    WHERE 1 = 0
)
;
"#;
        let expected = r#"
create or replace temporary table abc_db.abc_production_models_elementary.dbt_sources__tmp_20240102030405060708
as (

    SELECT
        *
    FROM abc_db.abc_production_models_elementary.dbt_sources
    WHERE 1 = 0
)
;
"#;
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Dynamic tmp suffixes starting with a plausible year should be ignored"
        );
    }

    #[test]
    fn test_structural_union_ordering_equivalence() {
        let actual = r#"create or replace   view EDP_SILVER_PROD.SHOPIFY.stg_order_line_refund




  as (






        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'FIBERON' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_FIBERON"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_FIBERON"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'FIBERON' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'FYPON' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_FYPON"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_FYPON"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'FYPON' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'LARSON' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'LARSON' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MASTERLOCK' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_STORE"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_STORE"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MASTERLOCK' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MASTERLOCK_REPLACEMENTS_EU' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MASTERLOCK_REPLACEMENTS_EU' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MASTERLOCK_REPLACEMENTS_UK' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS_UK"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS_UK"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MASTERLOCK_REPLACEMENTS_UK' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MOEN' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MOEN' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_STORE"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_STORE"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE_OWNERSCLUB' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_OWNERS_CLUB"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_OWNERS_CLUB"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE_OWNERSCLUB' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE_REPLACEMENTS_CA' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_CA"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_CA"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE_REPLACEMENTS_CA' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE_REPLACEMENTS_US' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_US"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_US"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE_REPLACEMENTS_US' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'THERMATRU' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_THERMATRU"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_THERMATRU"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'THERMATRU' and origin = 'refund'
                )







  );
        "#;

        let expected = r#"create or replace   view EDP_SILVER_PROD.SHOPIFY.stg_order_line_refund




  as (






        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MASTERLOCK_REPLACEMENTS_EU' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MASTERLOCK_REPLACEMENTS_EU' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MASTERLOCK_REPLACEMENTS_UK' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS_UK"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_REPLACEMENTS_UK"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MASTERLOCK_REPLACEMENTS_UK' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_STORE"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_STORE"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE_OWNERSCLUB' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_OWNERS_CLUB"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_OWNERS_CLUB"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE_OWNERSCLUB' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE_REPLACEMENTS_CA' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_CA"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_CA"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE_REPLACEMENTS_CA' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'SENTRYSAFE_REPLACEMENTS_US' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_US"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_SENTRYSAFE_REPLACEMENTS_US"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'SENTRYSAFE_REPLACEMENTS_US' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MASTERLOCK' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_STORE"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MASTERLOCK_STORE"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MASTERLOCK' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'FIBERON' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_FIBERON"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_FIBERON"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'FIBERON' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'FYPON' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_FYPON"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_FYPON"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'FYPON' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'THERMATRU' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_THERMATRU"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_THERMATRU"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'THERMATRU' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'LARSON' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'LARSON' and origin = 'refund'
                )



            union








        select
            olr.order_line_id,
            ol.order_id,
            ol.product_id,
            ol.sku,
            ol.variant_id,
            'refund' as origin,
            ol.price,
            -1 * olr.quantity,
            -1 * subtotal as line_total,
            'MOEN' as store,
            olr._fivetran_synced as created_at
        from "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ORDER_LINE_REFUND" olr
        left join "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ORDER_LINE" ol on ol.id = olr.order_line_id

            where
                created_at > (
                    select nvl(max(created_at), to_date('1900-01-01'))
                    from "EDP_SILVER_PROD"."SHOPIFY"."ORDER_LINE"
                    where store = 'MOEN' and origin = 'refund'
                )







  );
        "#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should treat union sets equal regardless of order within the CTE body"
        );
    }

    #[test]
    fn test_structural_union_ordering_equivalence_with_transient_extra_space() {
        let actual = r#"create or replace transient  table EDP_SILVER_PROD.SHOPIFY.abandoned_checkout_deleted

    as (

select id as checkout_id, 'MOEN' as origin from "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    union

select id as checkout_id, 'LARSON' as origin from "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    );"#;

        let expected = r#"create or replace transient table EDP_SILVER_PROD.SHOPIFY.abandoned_checkout_deleted

    as (

select id as checkout_id, 'LARSON' as origin from "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    union

select id as checkout_id, 'MOEN' as origin from "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    );"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should treat union sets equal regardless of order within the CTE body"
        );
    }

    #[test]
    fn test_structural_union_ordering_equivalence_with_temporary_extra_space() {
        let actual = r#"create or replace temporary  table EDP_SILVER_PROD.SHOPIFY.abandoned_checkout_deleted

    as (

select id as checkout_id, 'MOEN' as origin from "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    union

select id as checkout_id, 'LARSON' as origin from "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    );"#;

        let expected = r#"create or replace temporary table EDP_SILVER_PROD.SHOPIFY.abandoned_checkout_deleted

    as (

select id as checkout_id, 'LARSON' as origin from "EDP_BRONZE_PROD"."SHOPIFY_LARSON"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    union

select id as checkout_id, 'MOEN' as origin from "EDP_BRONZE_PROD"."SHOPIFY_MOEN"."ABANDONED_CHECKOUT"
where _FIVETRAN_DELETED = 'TRUE'

    );"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should treat union sets equal regardless of order within the CTE body"
        );
    }

    #[test]
    fn test_structural_union_ordering_equivalence_with_dashdash_comment() {
        let actual = r#"create or replace   view EDP_SILVER_PROD.SHOPIFY.stg_order_adjustment




  as (
    --

        select order_id, amount, 'FIBERON' as store
        from "EDP_BRONZE_PROD"."SHOPIFY_FIBERON"."ORDER_ADJUSTMENT"


            union








        select order_id, amount, 'FYPON' as store
        from "EDP_BRONZE_PROD"."SHOPIFY_FYPON"."ORDER_ADJUSTMENT"

  );"#;

        let expected = r#"create or replace   view EDP_SILVER_PROD.SHOPIFY.stg_order_adjustment




  as (
    --

        select order_id, amount, 'FYPON' as store
        from "EDP_BRONZE_PROD"."SHOPIFY_FYPON"."ORDER_ADJUSTMENT"


            union








        select order_id, amount, 'FIBERON' as store
        from "EDP_BRONZE_PROD"."SHOPIFY_FIBERON"."ORDER_ADJUSTMENT"

  );"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should treat union sets equal regardless of order within the CTE body"
        );
    }

    #[test]
    fn test_structural_union_all_ordering_equivalence() {
        let actual = r#"select * from (




with filtered_information_schema_columns as (


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('aftership')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_production_staging')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('google_ads')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('google_analytics')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_production')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('klaviyo')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('macroeconomic_data')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('mailchimp')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from machine_learning.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('predictions')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('mongodb')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('postgres_rds')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_schema')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('resmagic_api')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('returnly')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('sendgrid')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('shopify')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('information_schema')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('stripe')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('zendesk')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('zucc_meta')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_production_models')

)




)

select *
from filtered_information_schema_columns
where full_table_name is not null
    ) as __dbt_sbq
    where false
    limit 0
        "#;

        let expected = r#"select * from (




with filtered_information_schema_columns as (


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('aftership')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_production_staging')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('google_ads')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('google_analytics')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_production')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('klaviyo')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('macroeconomic_data')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('mailchimp')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from machine_learning.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('predictions')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_schema')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('returnly')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('sendgrid')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('information_schema')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('shopify')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('stripe')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('zendesk')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('zucc_meta')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('mongodb')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('postgres_rds')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from raw.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('resmagic_api')

)

            union all


        (


    select
        upper(table_catalog || '.' || table_schema || '.' || table_name) as full_table_name,
        upper(table_catalog) as database_name,
        upper(table_schema) as schema_name,
        upper(table_name) as table_name,
        upper(column_name) as column_name,
        data_type
    from iamcurious_db.INFORMATION_SCHEMA.COLUMNS
    where upper(table_schema) = upper('iamcurious_production_models')

)




)

select *
from filtered_information_schema_columns
where full_table_name is not null
    ) as __dbt_sbq
    where false
    limit 0
        "#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Should treat union-all sets equal regardless of order within the CTE body"
        );
    }

    #[test]
    fn test_compare_sql_dollar_quoted_typographic_quotes_ignored() {
        let actual = r#""ORDER_TYPE" COMMENT $$If is_renewal flag is set to 'TRUE' then we are tagging them as 'RENEWAL ORDER'.If it is set to false, but it is a later transaction of Credit type then it is called a "REFUND ORDER" else "FIRST ORDER"$$"#;
        let expected = r#""ORDER_TYPE" COMMENT $$If is_renewal flag is set to 'TRUE' then we are tagging them as 'RENEWAL ORDER'.If it is set to false, but it is a later transaction of Credit type then it is called a “REFUND ORDER” else “FIRST ORDER”$$"#;
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Typographic quotes inside dollar-quoted strings should be ignored"
        );
    }

    #[test]
    fn test_compare_sql_elementary_pkg_version_drift_ignored() {
        let actual = r#"
create or replace transient  table DB_FANANALYTICS.analytics_elementary_metadata.metadata



    as (

SELECT
    '0.21.0' as dbt_pkg_version
    )
;
"#;
        let expected = r#"
create or replace transient table DB_FANANALYTICS.analytics_elementary_metadata.metadata



    as (

SELECT
    '0.20.1' as dbt_pkg_version
    )
;
"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Elementary metadata package version drift should be ignored"
        );
    }

    #[test]
    fn test_compare_sql_elementary_pkg_version_drift_ignored_with_renamed_schema() {
        // Regression: some projects materialize Elementary's `metadata` model into the project's
        // target schema (or other renamed schemas), not `analytics_elementary_metadata`.
        let actual = r#"
create or replace transient  table SAM_CLARK_SANDBOX.weather_analytics_prd.metadata
    as (
SELECT
    '0.21.0' as dbt_pkg_version
    )
;
"#;
        let expected = r#"
create or replace transient table SAM_CLARK_SANDBOX.weather_analytics_prd.metadata
    as (
SELECT
    '0.20.1' as dbt_pkg_version
    )
;
"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Elementary metadata package version drift should be ignored even when schema is renamed"
        );
    }

    #[test]
    fn test_compare_sql_dbt_version_literal_drift_ignored() {
        // dbt1405: replay engine version and recorded engine version differ by construction.
        let actual = "select current_timestamp() as ts, '2.0.0' as dbt_version";
        let expected = "select current_timestamp() as ts, '2026.8.4+2f16d1c' as dbt_version";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "dbt_version literal drift should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_run_started_at_literal_drift_ignored() {
        // dbt1405: a customer macro deriving a literal from run_started_at differs when replay
        // happens at a different wall-clock time than the recording.
        let actual = "select '2026-08-09T16:01:19' as dbt_run_started_at";
        let expected = "select '2026-08-09T16:00:26' as dbt_run_started_at";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "run_started_at literal drift should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_dbt_version_literal_with_cast_drift_ignored() {
        // dbt1405: same as above, but the literal is explicitly cast before the alias.
        let actual = "select '2.0.0'::varchar as dbt_version";
        let expected = "select '2026.8.4+2f16d1c'::varchar as dbt_version";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "dbt_version literal drift with a cast should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_run_started_at_literal_with_cast_drift_ignored() {
        // dbt1405: same as above, but the literal is explicitly cast before the alias.
        let actual = "select '2026-08-09T16:01:19'::timestamp_ntz as dbt_run_started_at";
        let expected = "select '2026-08-09T16:00:26'::timestamp_ntz as dbt_run_started_at";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "run_started_at literal drift with a cast should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_etl_batch_id_literal_drift_ignored() {
        // dbt1405: a customer macro deriving an etl_batch_id from the run's wall-clock time.
        let actual = "select 20260808051943::number as etl_batch_id";
        let expected = "select 20260808050939::number as etl_batch_id";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "etl_batch_id literal drift should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_etl_batch_id_literal_with_other_cast_drift_ignored() {
        // dbt1405: same as above, but cast to a type other than `number`.
        let actual = "select 20260808051943::bigint as etl_batch_id";
        let expected = "select 20260808050939::bigint as etl_batch_id";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "etl_batch_id literal drift with a non-number cast should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_backup_relation_timestamp_suffix_drift_ignored() {
        // dbt1405: a custom materialization naming its backup relation with
        // `py_current_timestring()`, which replay re-evaluates, so the fresh suffix can never
        // equal the recorded one. The 23-digit actual is what preview.212 rendered, before #13684
        // fixed `%f` to Python's microsecond precision; masking has to cover both widths.
        let actual = "CREATE OR REPLACE TABLE REPRO_DATABASE.REPRO_SCHEMA.backup_probe_DBT_BACKUP_20260824102757911107255\nCLONE REPRO_DATABASE.REPRO_SCHEMA.source_probe";
        let expected = "CREATE OR REPLACE TABLE REPRO_DATABASE.REPRO_SCHEMA.backup_probe_DBT_BACKUP_20000101000000000000\nCLONE REPRO_DATABASE.REPRO_SCHEMA.source_probe";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "backup relation timestamp suffix drift should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_backup_relation_lowercase_suffix_drift_ignored() {
        // Same shape, but with the `__dbt_backup_` spelling dbt's own naming convention uses.
        let actual = "create or replace table analytics.sch.orders__dbt_backup_20260824102757911107255 clone analytics.sch.orders";
        let expected = "create or replace table analytics.sch.orders__dbt_backup_20240101000000000000 clone analytics.sch.orders";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "lowercase backup relation timestamp suffix drift should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_backup_relation_source_drift_still_fails() {
        // Guardrail: masking the suffix must not hide a different clone source.
        let actual = "create or replace table analytics.sch.orders__dbt_backup_20260824102757911107255 clone analytics.sch.orders";
        let expected = "create or replace table analytics.sch.orders__dbt_backup_20240101000000000000 clone analytics.sch.customers";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "a different clone source must still be reported as a mismatch"
        );
    }

    #[test]
    fn test_compare_sql_backup_relation_non_timestamp_suffix_drift_still_fails() {
        // Guardrail: only plausible `py_current_timestring()` values are masked, so a short
        // numeric suffix (e.g. a user-chosen batch number) still has to match.
        let actual = "create or replace table analytics.sch.orders__dbt_backup_17 clone analytics.sch.orders";
        let expected = "create or replace table analytics.sch.orders__dbt_backup_42 clone analytics.sch.orders";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "a non-timestamp backup suffix must still be reported as a mismatch"
        );
    }

    fn compact_timestamp_temp_view(
        run_started_at: &str,
        alias: &str,
        lhs: &str,
        rhs: &str,
        format: &str,
    ) -> String {
        format!(
            "create or replace temporary view timestamp_predicate as (\n\
             select '{run_started_at}' as {alias}\n\
             where TO_TIMESTAMP('{lhs}', '{format}')\n\
             < TO_TIMESTAMP('{rhs}', '{format}')\n\
             )"
        )
    }

    #[test]
    fn test_compare_sql_unaliased_to_timestamp_literal_drift_ignored() {
        let actual = compact_timestamp_temp_view(
            "2026-08-24T18:03:17+00:00",
            "dbt_run_started_at",
            "20000101000000",
            "20260824180317",
            "YYYYMMDDHH24MISSFF3",
        );
        let expected = compact_timestamp_temp_view(
            "2024-01-01T01:01:01+00:00",
            "dbt_run_started_at",
            "20000101000000",
            "20240101010101",
            "YYYYMMDDHH24MISSFF3",
        );

        let result = compare_sql(&actual, &expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "unaliased TO_TIMESTAMP literal drift should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_unaliased_to_timestamp_other_format_drift_preserved() {
        let actual = compact_timestamp_temp_view(
            "2026-08-24T18:03:17+00:00",
            "dbt_run_started_at",
            "20000101",
            "20260824",
            "YYYYMMDD",
        );
        let expected = compact_timestamp_temp_view(
            "2024-01-01T01:01:01+00:00",
            "dbt_run_started_at",
            "20000101",
            "20240101",
            "YYYYMMDD",
        );

        let result = compare_sql(&actual, &expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "compact timestamp drift with another format should remain significant"
        );
    }

    #[test]
    fn test_compare_sql_unaliased_to_timestamp_fixed_cutoff_drift_preserved() {
        let actual = compact_timestamp_temp_view(
            "2026-08-24T18:03:17+00:00",
            "dbt_run_started_at",
            "20000101000000",
            "20260824180317",
            "YYYYMMDDHH24MISSFF3",
        );
        let expected = compact_timestamp_temp_view(
            "2024-01-01T01:01:01+00:00",
            "dbt_run_started_at",
            "19990101000000",
            "20240101010101",
            "YYYYMMDDHH24MISSFF3",
        );

        let result = compare_sql(&actual, &expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "fixed TO_TIMESTAMP predicate cutoff drift should remain significant"
        );
    }

    #[test]
    fn test_compare_sql_unaliased_to_timestamp_hardcoded_rhs_drift_preserved() {
        let actual = compact_timestamp_temp_view(
            "2026-08-24T18:03:17+00:00",
            "dbt_run_started_at",
            "20000101000000",
            "20990101000000",
            "YYYYMMDDHH24MISSFF3",
        );
        let expected = compact_timestamp_temp_view(
            "2024-01-01T01:01:01+00:00",
            "dbt_run_started_at",
            "20000101000000",
            "20980101000000",
            "YYYYMMDDHH24MISSFF3",
        );

        let result = compare_sql(&actual, &expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "hardcoded TO_TIMESTAMP upper-bound drift should remain significant"
        );
    }

    #[test]
    fn test_compare_sql_unaliased_to_timestamp_plain_alias_drift_preserved() {
        let actual = compact_timestamp_temp_view(
            "2026-08-24T18:03:17+00:00",
            "run_started_at",
            "20000101000000",
            "20260824180317",
            "YYYYMMDDHH24MISSFF3",
        );
        let expected = compact_timestamp_temp_view(
            "2024-01-01T01:01:01+00:00",
            "run_started_at",
            "20000101000000",
            "20240101010101",
            "YYYYMMDDHH24MISSFF3",
        );

        let result = compare_sql(&actual, &expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "TO_TIMESTAMP drift should bind only to dbt_run_started_at"
        );
    }

    #[test]
    fn test_compare_sql_unaliased_to_timestamp_drift_outside_temp_view_preserved() {
        let actual = r#"
select '2026-08-24T18:03:17+00:00' as dbt_run_started_at
where TO_TIMESTAMP('20000101000000', 'YYYYMMDDHH24MISSFF3')
  < TO_TIMESTAMP('20260824180317', 'YYYYMMDDHH24MISSFF3')"#;
        let expected = r#"
select '2024-01-01T01:01:01+00:00' as dbt_run_started_at
where TO_TIMESTAMP('20000101000000', 'YYYYMMDDHH24MISSFF3')
  < TO_TIMESTAMP('20240101010101', 'YYYYMMDDHH24MISSFF3')"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_err(),
            "TO_TIMESTAMP drift outside temporary-view DDL should remain significant"
        );
    }

    #[test]
    fn test_compare_sql_unaliased_to_timestamp_drift_for_other_adapter_preserved() {
        let actual = compact_timestamp_temp_view(
            "2026-08-24T18:03:17+00:00",
            "dbt_run_started_at",
            "20000101000000",
            "20260824180317",
            "YYYYMMDDHH24MISSFF3",
        );
        let expected = compact_timestamp_temp_view(
            "2024-01-01T01:01:01+00:00",
            "dbt_run_started_at",
            "20000101000000",
            "20240101010101",
            "YYYYMMDDHH24MISSFF3",
        );

        let result = compare_sql(&actual, &expected, AdapterType::Databricks);
        assert!(
            result.is_err(),
            "TO_TIMESTAMP drift for a non-Snowflake adapter should remain significant"
        );
    }

    #[test]
    fn test_compare_sql_archive_stage_path_timestamp_drift_ignored() {
        // dbt1405: a customer macro deriving an archival stage path from the run's wall-clock time.
        let actual = "COPY FILES INTO '@DB.STG.TEMP_STAGE_FOR_ARCHIVAL/archive/20260808_180232/' FROM @DB.STG.TEMP_STAGE_FOR_ARCHIVAL";
        let expected = "COPY FILES INTO '@DB.STG.TEMP_STAGE_FOR_ARCHIVAL/archive/20260808_180104/' FROM @DB.STG.TEMP_STAGE_FOR_ARCHIVAL";
        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "archive stage path timestamp drift should be ignored: {result:?}"
        );
    }

    #[test]
    fn test_compare_sql_elementary_metadata_comment_ignored() {
        let actual = r#"select metadata_hash
    from OPERATIONS_PRD.MFG_INSTRUMENTS.dbt_exposures
    order by metadata_hash
    /* --ELEMENTARY-METADATA-- {"invocation_id": "019ba036-aec2-71a3-8709-a31b68ced8b0", "command": "build", "package_name": "elementary", "resource_name": "dbt_exposures", "resource_type": "model"} --END-ELEMENTARY-METADATA-- */"#;

        let expected = r#"select metadata_hash
    from OPERATIONS_PRD.MFG_INSTRUMENTS.dbt_exposures
    order by metadata_hash"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Elementary metadata comments should be ignored"
        );
    }

    #[test]
    fn test_compare_sql_nested_redundant_dbt_ctes_equivalent() {
        // Query with nested redundant dbt CTE definition
        let sql_with_nested_cte = r#"
with __dbt__cte__abc as (
    select a from A
),
__dbt__cte__efg as (
    with __dbt__cte__abc as (
        select a from A
    )
    select abc.a as a from __dbt__cte__abc
)
select abc.a from __dbt__cte__abc, efg.a from __dbt__cte__efg
"#;

        // Query that reuses outer dbt CTE
        let sql_reusing_outer_cte = r#"
with __dbt__cte__abc as (
    select a from A
),
__dbt__cte__efg as (
    select abc.a as a from __dbt__cte__abc
)
select abc.a from __dbt__cte__abc, efg.a from __dbt__cte__efg
"#;

        let result = compare_sql(
            sql_with_nested_cte,
            sql_reusing_outer_cte,
            AdapterType::Snowflake,
        );
        assert!(
            result.is_ok(),
            "Nested redundant dbt CTEs should be treated as equivalent to outer CTE reuse"
        );

        // Also test in reverse order
        let result_reversed = compare_sql(
            sql_reusing_outer_cte,
            sql_with_nested_cte,
            AdapterType::Snowflake,
        );
        assert!(result_reversed.is_ok(), "Comparison should be symmetric");
    }

    #[test]
    fn test_compare_sql_nested_dbt_ctes_different_definitions() {
        // Query with nested dbt CTE that has DIFFERENT definition
        let sql_with_different_nested = r#"
with __dbt__cte__abc as (
    select a from A
),
__dbt__cte__efg as (
    with __dbt__cte__abc as (
        select b from B
    )
    select abc.a as a from __dbt__cte__abc
)
select abc.a from __dbt__cte__abc, efg.a from __dbt__cte__efg
"#;

        // Query that reuses outer dbt CTE
        let sql_reusing_outer_cte = r#"
with __dbt__cte__abc as (
    select a from A
),
__dbt__cte__efg as (
    select abc.a as a from __dbt__cte__abc
)
select abc.a from __dbt__cte__abc, efg.a from __dbt__cte__efg
"#;

        let result = compare_sql(
            sql_with_different_nested,
            sql_reusing_outer_cte,
            AdapterType::Snowflake,
        );
        assert!(
            result.is_err(),
            "Nested dbt CTEs with different definitions should NOT be treated as equivalent"
        );
    }

    #[test]
    fn test_compare_sql_multiple_nested_redundant_dbt_ctes() {
        // Query with multiple nested redundant dbt CTEs
        let sql_with_nested = r#"
with __dbt__cte__abc as (
    select a from A
),
__dbt__cte__def as (
    select d from D
),
__dbt__cte__efg as (
    with __dbt__cte__abc as (
        select a from A
    ),
    __dbt__cte__def as (
        select d from D
    )
    select abc.a, def.d from __dbt__cte__abc, __dbt__cte__def
)
select * from __dbt__cte__efg
"#;

        // Query that reuses outer dbt CTEs
        let sql_reusing = r#"
with __dbt__cte__abc as (
    select a from A
),
__dbt__cte__def as (
    select d from D
),
__dbt__cte__efg as (
    select abc.a, def.d from __dbt__cte__abc, __dbt__cte__def
)
select * from __dbt__cte__efg
"#;

        let result = compare_sql(sql_with_nested, sql_reusing, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Multiple nested redundant dbt CTEs should be treated as equivalent"
        );
    }

    #[test]
    fn test_canonicalize_redundant_nested_dbt_ctes() {
        let sql_with_nested = r#"with __dbt__cte__abc as (select a from A), __dbt__cte__efg as (with __dbt__cte__abc as (select a from A) select abc.a from __dbt__cte__abc) select * from __dbt__cte__efg"#;

        let canonicalized = canonicalize_redundant_nested_ctes(sql_with_nested);

        // The canonicalized version should not contain the nested "with __dbt__cte__abc"
        assert!(
            !canonicalized.to_lowercase().contains(
                "with __dbt__cte__abc as (select a from a) select abc.a from __dbt__cte__abc"
            ),
            "Redundant nested dbt CTE should be removed. Got: {canonicalized}"
        );
    }

    #[test]
    fn test_compare_sql_deeply_nested_redundant_dbt_ctes() {
        // Query with deeply nested redundant dbt CTEs (3 levels)
        // - __dbt__cte__abc is defined at outer level
        // - __dbt__cte__hij is defined inside __dbt__cte__efg
        // - __dbt__cte__lll re-defines __dbt__cte__hij inside it (redundant)
        let sql_with_deep_nesting = r#"
with __dbt__cte__abc as (select a from A),
__dbt__cte__efg as (
    with __dbt__cte__abc as (select a from A),
    __dbt__cte__hij as (select b from B),
    __dbt__cte__lll as (
        with __dbt__cte__hij as (select b from B)
        select hij.b as b from __dbt__cte__hij
    )
    select abc.a, hij.b, lll.b from __dbt__cte__abc, __dbt__cte__hij, __dbt__cte__lll
)
select * from __dbt__cte__abc, __dbt__cte__efg
"#;

        // Query that reuses dbt CTEs from parent scopes
        let sql_reusing_parent_scopes = r#"
with __dbt__cte__abc as (select a from A),
__dbt__cte__efg as (
    with __dbt__cte__hij as (select b from B),
    __dbt__cte__lll as (select hij.b as b from __dbt__cte__hij)
    select abc.a, hij.b, lll.b from __dbt__cte__abc, __dbt__cte__hij, __dbt__cte__lll
)
select * from __dbt__cte__abc, __dbt__cte__efg
"#;

        let result = compare_sql(
            sql_with_deep_nesting,
            sql_reusing_parent_scopes,
            AdapterType::Snowflake,
        );
        assert!(
            result.is_ok(),
            "Deeply nested redundant dbt CTEs should be treated as equivalent"
        );

        // Also test in reverse order
        let result_reversed = compare_sql(
            sql_reusing_parent_scopes,
            sql_with_deep_nesting,
            AdapterType::Snowflake,
        );
        assert!(result_reversed.is_ok(), "Comparison should be symmetric");
    }

    #[test]
    fn test_compare_sql_deeply_nested_dbt_ctes_different_at_inner_level() {
        // Query where the deeply nested dbt CTE has a DIFFERENT definition
        let sql_with_different_deep_nested = r#"
with __dbt__cte__abc as (select a from A),
__dbt__cte__efg as (
    with __dbt__cte__hij as (select b from B),
    __dbt__cte__lll as (
        with __dbt__cte__hij as (select c from C)
        select hij.c as c from __dbt__cte__hij
    )
    select abc.a, hij.b, lll.c from __dbt__cte__abc, __dbt__cte__hij, __dbt__cte__lll
)
select * from __dbt__cte__abc, __dbt__cte__efg
"#;

        // Query that reuses __dbt__cte__hij from parent scope
        let sql_reusing_parent = r#"
with __dbt__cte__abc as (select a from A),
__dbt__cte__efg as (
    with __dbt__cte__hij as (select b from B),
    __dbt__cte__lll as (select hij.b as b from __dbt__cte__hij)
    select abc.a, hij.b, lll.b from __dbt__cte__abc, __dbt__cte__hij, __dbt__cte__lll
)
select * from __dbt__cte__abc, __dbt__cte__efg
"#;

        let result = compare_sql(
            sql_with_different_deep_nested,
            sql_reusing_parent,
            AdapterType::Snowflake,
        );
        assert!(
            result.is_err(),
            "Nested dbt CTEs with different definitions at inner levels should NOT be equivalent"
        );
    }

    #[test]
    fn test_compare_sql_sibling_dbt_cte_reference() {
        // Test that sibling dbt CTEs at the same level are properly in scope
        // Query where a CTE references a sibling CTE and later re-defines it redundantly
        let sql_with_redundant_sibling = r#"
with __dbt__cte__abc as (select a from A),
__dbt__cte__def as (select d from D),
__dbt__cte__efg as (
    with __dbt__cte__abc as (select a from A),
    __dbt__cte__def as (select d from D),
    __dbt__cte__ghi as (
        with __dbt__cte__def as (select d from D)
        select def.d from __dbt__cte__def
    )
    select abc.a, def.d, ghi.d from __dbt__cte__abc, __dbt__cte__def, __dbt__cte__ghi
)
select * from __dbt__cte__efg
"#;

        let sql_without_redundant = r#"
with __dbt__cte__abc as (select a from A),
__dbt__cte__def as (select d from D),
__dbt__cte__efg as (
    with __dbt__cte__ghi as (select def.d from __dbt__cte__def)
    select abc.a, def.d, ghi.d from __dbt__cte__abc, __dbt__cte__def, __dbt__cte__ghi
)
select * from __dbt__cte__efg
"#;

        let result = compare_sql(
            sql_with_redundant_sibling,
            sql_without_redundant,
            AdapterType::Snowflake,
        );
        assert!(
            result.is_ok(),
            "Sibling dbt CTE references should be properly handled"
        );
    }

    #[test]
    fn test_non_dbt_ctes_not_normalized() {
        // Test that user-defined CTEs (without __dbt__cte__ prefix) are NOT normalized
        // Even if they have the same definition, they should be kept as-is
        let sql_with_nested_user_cte = r#"
with user_cte as (
    select a from A
),
outer_cte as (
    with user_cte as (
        select a from A
    )
    select a from user_cte
)
select * from outer_cte
"#;

        let sql_without_nested = r#"
with user_cte as (
    select a from A
),
outer_cte as (
    select a from user_cte
)
select * from outer_cte
"#;

        let result = compare_sql(
            sql_with_nested_user_cte,
            sql_without_nested,
            AdapterType::Snowflake,
        );
        // This should NOT be equivalent because user CTEs are not normalized
        assert!(
            result.is_err(),
            "Non-dbt CTEs should NOT be normalized - they are user-defined and should be preserved"
        );
    }

    #[test]
    fn test_compare_sql_python_config_dict_with_nested_quotes() {
        // Test that config_dict with nested quotes (like env_var template syntax) is properly canonicalized
        let actual = r#"config_dict = {'meta': {'indirect_selection': "{{ env_var('DBT_INDIRECT_SELECTION', 'cautious') }}", 'dbt_environment': 'Prod'}, 'database': 'REPORTING_PROD', 'schema': 'exports', 'backup_config': {}}"#;
        let expected = r#"config_dict = {'meta': {'dbt_environment': 'Prod'}, 'database': 'REPORTING_PROD', 'schema': 'exports', 'backup_config': {}}"#;

        let canonicalized = canonicalize_python_config_dict(actual, expected);

        // Should canonicalize to the expected format (ours has a superset meta)
        assert!(
            canonicalized.contains("{'dbt_environment': 'Prod'}"),
            "Should canonicalize to expected config_dict, got: {}",
            canonicalized
        );
        assert!(
            !canonicalized.contains("indirect_selection"),
            "Should remove the indirect_selection key"
        );
    }

    #[test]
    fn test_compare_sql_python_config_dict_different_non_meta_keys() {
        // Test that config_dict is NOT canonicalized when non-meta keys differ
        let actual = r#"config_dict = {'meta': {'indirect_selection': 'cautious', 'dbt_environment': 'Prod'}, 'database': 'REPORTING_DEV', 'schema': 'exports', 'backup_config': {}}"#;
        let expected = r#"config_dict = {'meta': {'dbt_environment': 'Prod'}, 'database': 'REPORTING_PROD', 'schema': 'exports', 'backup_config': {}}"#;

        let canonicalized = canonicalize_python_config_dict(actual, expected);

        // Should NOT canonicalize because database differs (REPORTING_DEV vs REPORTING_PROD)
        assert!(
            canonicalized.contains("REPORTING_DEV"),
            "Should preserve actual's database when non-meta keys differ, got: {}",
            canonicalized
        );
        assert!(
            canonicalized.contains("indirect_selection"),
            "Should preserve actual's extra meta keys when non-meta keys differ"
        );
    }

    #[test]
    fn test_numeric_and_decimal_are_equivalent() {
        // Databricks (and SQL standard) treat NUMERIC and DECIMAL as synonyms.
        // Fusion may emit `numeric(28,6)` while Mantle recordings have `decimal(28,6)`.
        // These should be treated as equivalent during SQL comparison.
        let actual = "create or replace table `dbt`.`dbt_staging`.`my_model`
as
select
    cast(null as numeric(28,6)) as amount,
    cast(null as string) as name
from dummy_cte
where 1 = 0";

        let expected = "create or replace table `dbt`.`dbt_staging`.`my_model`
as
select
    cast(null as decimal(28,6)) as amount,
    cast(null as string) as name
from dummy_cte
where 1 = 0";

        let result = compare_sql(actual, expected, AdapterType::Databricks);
        assert!(
            result.is_ok(),
            "numeric(28,6) and decimal(28,6) should be treated as equivalent: {result:?}"
        );
    }

    #[test]
    fn test_alter_table_set_tblproperties_order_independent() {
        // Fusion and dbt-databricks may emit the same tblproperties in different order.
        // The properties are a set of key-value pairs — ordering is not semantically meaningful.
        let actual = r#"ALTER TABLE `dbt`.`dbt_staging`.`stg_aa_base_philosophy` SET
    tblproperties ('delta.columnMapping.mode' = 'name' , 'delta.enableChangeDataFeed' = 'true'
    )"#;

        let expected = r#"ALTER TABLE `dbt`.`dbt_staging`.`stg_aa_base_philosophy` SET
    tblproperties ('delta.enableChangeDataFeed' = 'true' , 'delta.columnMapping.mode' = 'name'
    )"#;

        let result = compare_sql(actual, expected, AdapterType::Databricks);
        assert!(
            result.is_ok(),
            "ALTER TABLE SET tblproperties should be order-independent: {result:?}"
        );
    }

    #[test]
    fn test_python_meta_dict_and_meta_get_are_ignorable() {
        // Fusion emits `meta_dict = {}` and a `meta_get` static method inside the
        // `config` helper class for Snowflake Python models.  Mantle does not.
        // The extra code is semantically inert and should not cause a mismatch.
        let actual = r#"
config_dict = {}
meta_dict = {}

class config:
    def __init__(self, *args, **kwargs):
        pass

    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

    @staticmethod
    def meta_get(key, default=None):
        return meta_dict.get(key, default)

class this:
    database = "DB"
"#;

        let expected = r#"
config_dict = {}

class config:
    def __init__(self, *args, **kwargs):
        pass

    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

class this:
    database = "DB"
"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "meta_dict and meta_get differences should be ignorable: {result:?}"
        );
    }

    #[test]
    fn test_python_nonempty_meta_dict_promoted_to_config_dict_and_meta_get_calls_rewritten() {
        // Regression test for: Fusion emits `config_dict = {}` (empty) + `meta_dict = {data}`
        // + `dbt.config.meta_get(...)` call sites in the model body, while Mantle emits
        // `config_dict = {data}` (populated from meta) + no meta_dict + `dbt.config.get(...)`
        // call sites. The two forms are semantically equivalent and should not cause a mismatch.
        //
        // This covers models like rte_followup_key_message, clinical_trial_start_end,
        // event_congress_activity, email_bounced, etc. that use dbt.config.meta_get() to read
        // values defined under `meta:` in their config.yml.
        let actual = r#"
config_dict = {}
meta_dict = {'days': {'ID': 7, 'TH': 7}, 'countries': ['ID', 'TH']}

class config:
    def __init__(self, *args, **kwargs):
        pass

    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

class this:
    database = "DB"
    schema = "SCH"
    identifier = "my_model"

def model(dbt, session):
    last_n_days = dbt.config.meta_get("days")
    countries = dbt.config.meta_get("countries")
    return session.table("something")
"#;

        let expected = r#"
config_dict = {'days': {'ID': 7, 'TH': 7}, 'countries': ['ID', 'TH']}

class config:
    def __init__(self, *args, **kwargs):
        pass

    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

class this:
    database = "DB"
    schema = "SCH"
    identifier = "my_model"

def model(dbt, session):
    last_n_days = dbt.config.get("days")
    countries = dbt.config.get("countries")
    return session.table("something")
"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "Populated meta_dict + meta_get call sites should be treated as equivalent \
             to populated config_dict + get call sites: {result:?}"
        );
    }

    #[test]
    fn test_canonicalize_python_model_pair_config_meta_equivalent() {
        // Databricks submit_python_job payload. dbt-autofix CustomKeyInConfigDeprecation rewrites
        // `dbt.config.get(<custom key>)` to `meta_get(...)` and moves the key from config_dict to
        // meta_dict. Fusion built the post-autofix source (meta_get + populated meta_dict), Core
        // recorded the pre-autofix source (get + populated config_dict). canonicalize_python_model_pair
        // must normalize the two to byte-equal so the exact compare in replay_submit_python_job passes.
        let fusion = r#"
def model(dbt, session):
    start_year = int(dbt.config.meta_get("start_year", 2020))
    end_year = int(dbt.config.meta_get("end_year", 2050))
    return session.table("x")

config_dict = {}
meta_dict = {'start_year': 2020, 'end_year': 2050}

class config:
    def __init__(self, *args, **kwargs):
        pass

    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

    @staticmethod
    def meta_get(key, default=None):
        return meta_dict.get(key, default)
"#;

        let core = r#"
def model(dbt, session):
    start_year = int(dbt.config.get("start_year", 2020))
    end_year = int(dbt.config.get("end_year", 2050))
    return session.table("x")

config_dict = {'start_year': 2020, 'end_year': 2050}
meta_dict = {}

class config:
    def __init__(self, *args, **kwargs):
        pass

    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

    @staticmethod
    def meta_get(key, default=None):
        return meta_dict.get(key, default)
"#;

        // Also exercise line-ending divergence: a CRLF source (Windows) is preserved by Core
        // and emitted as LF by Fusion, so every line would otherwise differ byte-for-byte.
        let core_crlf = core.replace('\n', "\r\n");
        let (a, e) = canonicalize_python_model_pair(fusion, &core_crlf);
        assert_eq!(
            a, e,
            "config/meta forms and CRLF vs LF line endings should canonicalize equal"
        );
    }

    #[test]
    fn test_python_meta_get_normalizes_both_sides() {
        // Some Mantle recordings preserve `dbt.config.meta_get(...)` in the
        // generated stored procedure body (rather than rewriting to `get`).
        // Fusion's actual passes the user's source verbatim, so it also has
        // `meta_get`. The canonicalizer must normalize BOTH sides — otherwise
        // the actual gets rewritten to `get` while the expected keeps
        // `meta_get`, causing a spurious mismatch.
        let actual = r#"
config_dict = {'k': 'v'}

class config:
    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

def model(dbt, session):
    x = dbt.config.meta_get("k")
    return session.table("t")
"#;

        let expected = r#"
config_dict = {'k': 'v'}

class config:
    @staticmethod
    def get(key, default=None):
        return config_dict.get(key, default)

def model(dbt, session):
    x = dbt.config.meta_get("k")
    return session.table("t")
"#;

        let result = compare_sql(actual, expected, AdapterType::Snowflake);
        assert!(
            result.is_ok(),
            "matching meta_get on both sides should compare equal: {result:?}"
        );
    }

    #[test]
    fn test_struct_projection_multibyte_char() {
        let sql_cjk = "SELECT struct(1 AS x) AS alpha, '日本語' AS account FROM t";
        let result = canonicalize_struct_projection_order_in_select_lists(sql_cjk);
        assert_eq!(result, sql_cjk);

        let sql_alias = "SELECT struct(1) AS 한국어alias, struct(2) AS beta FROM t";
        let result = canonicalize_struct_projection_order_in_select_lists(sql_alias);
        assert_eq!(result, sql_alias);
    }

    #[test]
    fn test_strip_sql_comments() {
        // Line comment: stripped, newline preserved to avoid gluing tokens.
        assert_eq!(strip_sql_comments("a -- comment\nb"), "a \nb");
        // Line comment at end of input with no newline.
        assert_eq!(strip_sql_comments("a -- comment"), "a ");
        // Block comment: stripped entirely.
        assert_eq!(strip_sql_comments("a /* comment */ b"), "a  b");
        // Block comment with no trailing content.
        assert_eq!(strip_sql_comments("a /* comment */"), "a ");

        // -- and /* inside single/double/backtick quotes are NOT stripped.
        assert_eq!(strip_sql_comments("'hello -- world'"), "'hello -- world'");
        assert_eq!(
            strip_sql_comments("\"hello /* world */\""),
            "\"hello /* world */\""
        );
        assert_eq!(strip_sql_comments("`col -- name`"), "`col -- name`");

        // Multi-byte characters in a string literal are preserved intact.
        assert_eq!(strip_sql_comments("'日本語'"), "'日本語'");
        assert_eq!(strip_sql_comments("'한국어'"), "'한국어'");

        // Multi-byte characters in a line comment are consumed, newline still kept.
        assert_eq!(strip_sql_comments("a -- 日本語\nb"), "a \nb");
        // Multi-byte characters in a block comment are consumed.
        assert_eq!(strip_sql_comments("a /* 日本語 */ b"), "a  b");
    }
}
