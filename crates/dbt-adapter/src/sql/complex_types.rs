//! Canonical spelling of Databricks/Spark type expressions.
//!
//! `struct<a:string,b:array<int>>` and ``STRUCT<`a` string, `b` ARRAY<int>>`` name the same
//! type, as do `decimal(38,18)` and `DECIMAL(38, 18)`. Rewriting both to one spelling lets
//! replay match SQL across the two forms.

use std::borrow::Cow;

use super::diff::is_word_byte;
use crate::AdapterType;

/// Canonicalize type expressions in `sql` for Databricks and Spark; other adapters
/// return `sql` unchanged.
pub(crate) fn canonicalize_complex_types_for(sql: &str, adapter_type: AdapterType) -> Cow<'_, str> {
    match adapter_type {
        AdapterType::Databricks | AdapterType::Spark => canonicalize_complex_types(sql),
        _ => Cow::Borrowed(sql),
    }
}

/// Rewrite every `struct<...>`, `array<...>`, `map<...>`, `decimal(p,s)` and `void` type
/// expression to lowercase keywords, `name:type` fields, unquoted plain field names and no
/// whitespace.
///
/// Text that does not parse as a type, such as `map < 3` or `decimal(amount)`, is left
/// unchanged, as are string literals, quoted identifiers and comments.
pub(crate) fn canonicalize_complex_types(sql: &str) -> Cow<'_, str> {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0;
    let mut i = 0;
    let mut changed = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i = bytes[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(bytes.len(), |n| i + n + 1);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = sql[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |n| i + 2 + n + 2);
            }
            c if is_word_byte(c) => {
                let start = i;
                while i < bytes.len() && is_word_byte(bytes[i]) {
                    i += 1;
                }
                if !is_type_keyword(&sql[start..i]) {
                    continue;
                }
                let mut parser = Parser {
                    sql,
                    pos: start,
                    depth: 0,
                };
                if let Some(canonical) = parser.parse_type() {
                    if sql[start..parser.pos] != canonical {
                        out.push_str(&sql[copied..start]);
                        out.push_str(&canonical);
                        copied = parser.pos;
                        changed = true;
                    }
                    i = parser.pos;
                }
            }
            _ => i += 1,
        }
    }
    if !changed {
        return Cow::Borrowed(sql);
    }
    out.push_str(&sql[copied..]);
    Cow::Owned(out)
}

#[derive(Clone, Copy)]
enum ComplexKeyword {
    Struct,
    Array,
    Map,
}

fn complex_keyword(word: &str) -> Option<ComplexKeyword> {
    if word.eq_ignore_ascii_case("struct") {
        Some(ComplexKeyword::Struct)
    } else if word.eq_ignore_ascii_case("array") {
        Some(ComplexKeyword::Array)
    } else if word.eq_ignore_ascii_case("map") {
        Some(ComplexKeyword::Map)
    } else {
        None
    }
}

/// Words that start a type expression this module canonicalizes.
fn is_type_keyword(word: &str) -> bool {
    complex_keyword(word).is_some()
        || word.eq_ignore_ascii_case("decimal")
        || word.eq_ignore_ascii_case("void")
}

/// Return the index just past the string literal or quoted identifier starting at `start`.
///
/// Strings use backslash escapes and backtick identifiers escape a backtick by doubling it,
/// which also ends and reopens the quote, so both forms are skipped correctly.
fn skip_quoted(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quote != b'`' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Deepest type nesting the parser follows, which bounds its recursion. Levels outside the
/// innermost `MAX_NESTING` are left unchanged.
const MAX_NESTING: usize = 128;

struct Parser<'a> {
    sql: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.sql.as_bytes().get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn word(&mut self) -> Option<&'a str> {
        let start = self.pos;
        while self.peek().is_some_and(is_word_byte) {
            self.pos += 1;
        }
        (self.pos > start).then(|| &self.sql[start..self.pos])
    }

    /// Parse a type and return its canonical spelling.
    fn parse_type(&mut self) -> Option<String> {
        self.skip_ws();
        let word = self.word()?;
        if !word.as_bytes()[0].is_ascii_alphabetic() {
            return None;
        }
        let Some(keyword) = complex_keyword(word) else {
            return self.parse_primitive_params(word.to_ascii_lowercase());
        };
        if self.depth >= MAX_NESTING || !self.eat(b'<') {
            return None;
        }
        self.depth += 1;
        let canonical = match keyword {
            ComplexKeyword::Array => format!("array<{}>", self.parse_type()?),
            ComplexKeyword::Map => {
                let key = self.parse_type()?;
                if !self.eat(b',') {
                    return None;
                }
                format!("map<{key},{}>", self.parse_type()?)
            }
            ComplexKeyword::Struct => {
                let mut fields = Vec::new();
                self.skip_ws();
                if self.peek() != Some(b'>') {
                    loop {
                        fields.push(self.parse_field()?);
                        if !self.eat(b',') {
                            break;
                        }
                    }
                }
                format!("struct<{}>", fields.join(","))
            }
        };
        self.depth -= 1;
        self.eat(b'>').then_some(canonical)
    }

    /// Parse optional `(n, m, ...)` parameters after a primitive type name.
    fn parse_primitive_params(&mut self, name: String) -> Option<String> {
        let after_name = self.pos;
        if !self.eat(b'(') {
            self.pos = after_name;
            return Some(name);
        }
        let mut params = Vec::new();
        loop {
            self.skip_ws();
            let start = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
            if self.pos == start {
                return None;
            }
            params.push(&self.sql[start..self.pos]);
            if !self.eat(b',') {
                break;
            }
        }
        self.eat(b')')
            .then(|| format!("{name}({})", params.join(",")))
    }

    /// Parse a `name:type` or `name type` struct field.
    fn parse_field(&mut self) -> Option<String> {
        self.skip_ws();
        let name = if self.peek() == Some(b'`') {
            self.quoted_name()?
        } else {
            self.word()?.to_string()
        };
        let before_sep = self.pos;
        if !self.eat(b':') {
            self.pos = before_sep;
            if !self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                return None;
            }
        }
        Some(format!("{name}:{}", self.parse_type()?))
    }

    /// Parse a backtick-quoted field name. Plain names lose their backticks.
    fn quoted_name(&mut self) -> Option<String> {
        let start = self.pos;
        self.pos += 1;
        loop {
            match self.peek()? {
                b'`' if self.sql.as_bytes().get(self.pos + 1) == Some(&b'`') => self.pos += 2,
                b'`' => break,
                _ => self.pos += 1,
            }
        }
        self.pos += 1;
        let quoted = &self.sql[start..self.pos];
        let inner = &quoted[1..quoted.len() - 1];
        if !inner.is_empty() && inner.bytes().all(is_word_byte) {
            Some(inner.to_string())
        } else {
            Some(quoted.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(sql: &str) -> String {
        canonicalize_complex_types(sql).into_owned()
    }

    #[test]
    fn colon_and_quoted_spellings_match() {
        let recorded = "CAST(NULL AS struct<host:string,scope:struct<env:string,region:string>,ts:string>) AS metadata";
        let actual = "CAST(NULL AS STRUCT<`host` string, `scope` STRUCT<`env` string, `region` string>, `ts` string>) AS metadata";
        assert_eq!(canon(recorded), canon(actual));
        assert_eq!(
            canon(recorded),
            "CAST(NULL AS struct<host:string,scope:struct<env:string,region:string>,ts:string>) AS metadata"
        );
    }

    #[test]
    fn arrays_maps_and_params_match() {
        let recorded = "CAST(NULL AS array<struct<id:bigint,amount:decimal(10,2),tags:map<string,array<int>>>>)";
        let actual = "CAST(NULL AS ARRAY<STRUCT<`id` bigint, `amount` DECIMAL(10, 2), `tags` MAP<string, ARRAY<int>>>>)";
        assert_eq!(canon(recorded), canon(actual));
    }

    #[test]
    fn field_names_keep_case_and_special_quoting() {
        assert_eq!(
            canon("struct<`hostName` string>"),
            "struct<hostName:string>"
        );
        assert_eq!(
            canon("STRUCT<`a b` string, `x``y` int>"),
            "struct<`a b`:string,`x``y`:int>"
        );
    }

    #[test]
    fn real_differences_still_differ() {
        let base = canon("struct<a:string,b:int>");
        for other in [
            "STRUCT<`a` string, `c` int>",
            "STRUCT<`a` string, `b` bigint>",
            "STRUCT<`b` int, `a` string>",
            "STRUCT<`A` string, `b` int>",
        ] {
            assert_ne!(base, canon(other), "{other}");
        }
    }

    #[test]
    fn non_types_and_literals_are_unchanged() {
        for sql in [
            "select map < 3, array<b and c> d from t",
            "select 'struct<`a` string>' as s",
            "select `struct` < 1",
            "select map_keys(m) from t where x <> 1",
            "select struct<a:string from t",
        ] {
            assert!(
                matches!(canonicalize_complex_types(sql), Cow::Borrowed(_)),
                "{sql}"
            );
        }
    }

    #[test]
    fn decimal_and_void_spellings_match() {
        let recorded = "select cast(null as void) as a, cast(1.0 as decimal(38,18)) as b";
        let actual = "select cast(null as VOID) as a, cast(1.0 as DECIMAL(38, 18)) as b";
        assert_eq!(canon(recorded), canon(actual));
        assert_eq!(canon(recorded), recorded);
        assert_ne!(
            canon("cast(1.0 as decimal(38,18))"),
            canon("cast(1.0 as DECIMAL(38, 6))")
        );
        for sql in ["select decimal(amount) from t", "select `decimal` from t"] {
            assert!(
                matches!(canonicalize_complex_types(sql), Cow::Borrowed(_)),
                "{sql}"
            );
        }
    }

    #[test]
    fn quotes_and_comments_do_not_stop_canonicalization() {
        for prefix in [
            "-- don't\n",
            "/* don't */ ",
            "select \"it's\" as s, ",
            "select `it's` as s, ",
            "select 'it\\'s' as s, ",
        ] {
            assert_eq!(
                canon(&format!("{prefix}cast(null as STRUCT<`a` string>)")),
                format!("{prefix}cast(null as struct<a:string>)"),
                "{prefix}"
            );
        }
    }

    #[test]
    fn quoted_and_commented_type_words_are_unchanged() {
        for sql in [
            "select `my DECIMAL(10, 2)` from t",
            "select \"VOID\" from t",
            "-- STRUCT<`a` string>\nselect 1",
            "/* ARRAY<int> */ select 1",
        ] {
            assert!(
                matches!(canonicalize_complex_types(sql), Cow::Borrowed(_)),
                "{sql}"
            );
        }
    }

    #[test]
    fn deep_nesting_keeps_outer_levels() {
        let deep = format!(
            "cast(null as {}int{})",
            "ARRAY<".repeat(1000),
            ">".repeat(1000)
        );
        let outer = format!("cast(null as {}", "ARRAY<".repeat(1000 - MAX_NESTING));
        assert!(canon(&deep).starts_with(&format!("{outer}array<")));
        let shallow = format!(
            "{}int{}",
            "ARRAY<".repeat(MAX_NESTING),
            ">".repeat(MAX_NESTING)
        );
        assert_eq!(
            canon(&shallow),
            format!(
                "{}int{}",
                "array<".repeat(MAX_NESTING),
                ">".repeat(MAX_NESTING)
            )
        );
    }

    #[test]
    fn only_databricks_and_spark_are_canonicalized() {
        let sql = "CAST(NULL AS STRUCT<`a` string>)";
        assert_eq!(
            canonicalize_complex_types_for(sql, AdapterType::Databricks),
            "CAST(NULL AS struct<a:string>)"
        );
        assert_eq!(
            canonicalize_complex_types_for(sql, AdapterType::Spark),
            "CAST(NULL AS struct<a:string>)"
        );
        assert_eq!(
            canonicalize_complex_types_for(sql, AdapterType::Bigquery),
            sql
        );
    }
}
