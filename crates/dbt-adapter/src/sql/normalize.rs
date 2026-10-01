use regex::Regex;
use std::sync::LazyLock;

/// Replace non-deterministic dbt temporary-table identifiers with a stable
/// canonical form so record/replay comparisons are order-independent.
///
/// Two producer patterns need to be handled — both call sites are dbt-adapters
/// upstream, dispatched via `make_temp_relation`:
///
/// 1. `adapter.generate_unique_temporary_table_suffix` (Athena/base) →
///    `dbt_tmp_<32-hex-with-underscores>` (UUIDv4 with dashes → underscores).
///    Example: `dbt_tmp_800c2fb4_a0ba_4708_a0b1_813316032bfb` → `dbt_tmp_`.
///
/// 2. `postgres__make_relation_with_suffix` (Postgres/Redshift) and
///    `bigquery__make_relation_with_suffix` (BigQuery) →
///    `<base>__dbt_tmp<digits>` where the digits are
///    `datetime.now().strftime("%H%M%S%f")` (12+ decimal digits, varies by
///    Jinja engine precision). Example:
///    `target__dbt_tmp102340081152082` → `target__dbt_tmp`.
///
/// The two patterns are disambiguated by prefix (single vs. double underscore)
/// and body (hex-with-underscores vs. digits-only), so they don't overlap.
pub fn normalize_dbt_tmp_name(sql: &str) -> String {
    static DBT_TMP_UUID_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"dbt_tmp_[0-9a-f]{8}_[0-9a-f]{4}_[0-9a-f]{4}_[0-9a-f]{4}_[0-9a-f]{12}").unwrap()
    });
    static DBT_TMP_TIMESTAMP_PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"__dbt_tmp\d+").unwrap());

    let step1 = DBT_TMP_UUID_PATTERN.replace_all(sql, "dbt_tmp_");
    DBT_TMP_TIMESTAMP_PATTERN
        .replace_all(&step1, "__dbt_tmp")
        .to_string()
}

pub fn strip_sql_comments(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut last_output_whitespace = true;
    let mut pending_space = false;

    while let Some(ch) = chars.next() {
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
                result.push('\n');
                last_output_whitespace = true;
                pending_space = false;
            }
            continue;
        }
        if in_block_comment {
            if ch == '*' && chars.peek().is_some_and(|next| *next == '/') {
                let _ = chars.next();
                in_block_comment = false;
            }
            continue;
        }

        if pending_space {
            if ch == '\n' || ch.is_whitespace() {
                pending_space = false;
            } else if !result.is_empty() {
                result.push(' ');
                last_output_whitespace = true;
                pending_space = false;
            }
        }

        if in_single {
            result.push(ch);
            if ch == '\'' {
                if chars.peek().is_some_and(|next| *next == '\'') {
                    result.push('\'');
                    let _ = chars.next();
                } else {
                    in_single = false;
                }
            }
            last_output_whitespace = false;
            continue;
        }

        if in_double {
            result.push(ch);
            if ch == '"' {
                if chars.peek().is_some_and(|next| *next == '"') {
                    result.push('"');
                    let _ = chars.next();
                } else {
                    in_double = false;
                }
            }
            last_output_whitespace = false;
            continue;
        }

        match ch {
            '-' if chars.peek().is_some_and(|next| *next == '-') => {
                let _ = chars.next();
                in_line_comment = true;
                if !last_output_whitespace && !result.is_empty() {
                    pending_space = true;
                }
            }
            '/' if chars.peek().is_some_and(|next| *next == '*') => {
                let _ = chars.next();
                in_block_comment = true;
                if !last_output_whitespace && !result.is_empty() {
                    pending_space = true;
                }
            }
            '\'' => {
                in_single = true;
                result.push('\'');
                last_output_whitespace = false;
            }
            '"' => {
                in_double = true;
                result.push('"');
                last_output_whitespace = false;
            }
            '\n' => {
                result.push('\n');
                last_output_whitespace = true;
                pending_space = false;
            }
            _ if ch.is_whitespace() => {
                if !last_output_whitespace {
                    result.push(' ');
                    last_output_whitespace = true;
                }
            }
            _ => {
                result.push(ch);
                last_output_whitespace = false;
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_dbt_tmp_name_basic() {
        let input = "SELECT * FROM dbt_tmp_800c2fb4_a0ba_4708_a0b1_813316032bfb";
        assert_eq!(normalize_dbt_tmp_name(input), "SELECT * FROM dbt_tmp_");
    }

    #[test]
    fn test_normalize_dbt_tmp_name_multiple_uuids() {
        let input = "INSERT INTO dbt_tmp_aaaaaaaa_bbbb_cccc_dddd_eeeeeeeeeeee \
                      SELECT * FROM dbt_tmp_11111111_2222_3333_4444_555555555555";
        assert_eq!(
            normalize_dbt_tmp_name(input),
            "INSERT INTO dbt_tmp_ SELECT * FROM dbt_tmp_"
        );
    }

    #[test]
    fn test_normalize_dbt_tmp_name_no_uuid() {
        let input = "SELECT 1";
        assert_eq!(normalize_dbt_tmp_name(input), "SELECT 1");
    }

    #[test]
    fn test_normalize_dbt_tmp_name_partial_uuid_unchanged() {
        let input = "SELECT * FROM dbt_tmp_800c2fb4";
        assert_eq!(normalize_dbt_tmp_name(input), input);
    }

    #[test]
    fn test_normalize_dbt_tmp_name_postgres_timestamp_suffix() {
        // Postgres/Redshift: `strftime("%H%M%S%f")` — 12 decimal digits.
        let input = "create temporary table \"target__dbt_tmp102033747450\"";
        assert_eq!(
            normalize_dbt_tmp_name(input),
            "create temporary table \"target__dbt_tmp\""
        );
    }

    #[test]
    fn test_normalize_dbt_tmp_name_fusion_timestamp_suffix() {
        // Fusion's Jinja emits higher-precision %f — 15 decimal digits observed.
        let input = "table_name = 'target__dbt_tmp102340081152082'";
        assert_eq!(
            normalize_dbt_tmp_name(input),
            "table_name = 'target__dbt_tmp'"
        );
    }

    #[test]
    fn test_normalize_dbt_tmp_name_multiple_timestamps_across_query() {
        // Same identifier repeated (SELECT-then-USING style in incremental merge).
        let input = "using \"foo__dbt_tmp073022229909\" join \"foo__dbt_tmp073022229909\"";
        assert_eq!(
            normalize_dbt_tmp_name(input),
            "using \"foo__dbt_tmp\" join \"foo__dbt_tmp\""
        );
    }

    #[test]
    fn test_normalize_dbt_tmp_name_double_underscore_partial_unchanged() {
        // `__dbt_tmp` with no digits is already canonical.
        let input = "select * from foo__dbt_tmp";
        assert_eq!(normalize_dbt_tmp_name(input), input);
    }

    #[test]
    fn test_normalize_dbt_tmp_name_idempotent() {
        // Applying twice yields the same output — matters for the record-mode
        // normalization path where SedTask may run this on already-normalized SQL.
        let input =
            "target__dbt_tmp102340081152082 and dbt_tmp_800c2fb4_a0ba_4708_a0b1_813316032bfb";
        let once = normalize_dbt_tmp_name(input);
        let twice = normalize_dbt_tmp_name(&once);
        assert_eq!(once, twice);
        assert_eq!(once, "target__dbt_tmp and dbt_tmp_");
    }

    #[test]
    fn test_strip_sql_comments_line_comment() {
        assert_eq!(
            strip_sql_comments("SELECT 1 -- comment\nFROM t"),
            "SELECT 1 \nFROM t"
        );
    }

    #[test]
    fn test_strip_sql_comments_block_comment() {
        assert_eq!(strip_sql_comments("SELECT /* inline */ 1"), "SELECT 1");
    }

    #[test]
    fn test_strip_sql_comments_preserves_string_literals() {
        assert_eq!(
            strip_sql_comments("SELECT '-- not a comment'"),
            "SELECT '-- not a comment'"
        );
    }
}
