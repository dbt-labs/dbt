use std::borrow::Cow;

/// Lowercases `input`, passing it through untouched when it is trivially
/// lowercase already.
pub fn lowercase<'a>(input: Cow<'a, str>) -> Cow<'a, str> {
    // Conservative check that `to_lowercase` would not change `input`. Stay
    // clear of Unicode's special cases, such as the contextual `Σ` mapping.
    let is_trivially_lowercase = input
        .bytes()
        .all(|b| b.is_ascii() && !b.is_ascii_uppercase());
    if is_trivially_lowercase {
        input
    } else {
        Cow::Owned(input.to_lowercase())
    }
}

/// Uppercases `input`, passing it through untouched when it is trivially
/// uppercase already.
pub fn uppercase<'a>(input: Cow<'a, str>) -> Cow<'a, str> {
    let is_trivially_uppercase = input
        .bytes()
        .all(|b| b.is_ascii() && !b.is_ascii_lowercase());
    if is_trivially_uppercase {
        input
    } else {
        Cow::Owned(input.to_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_lowercase(input: &str, expect_borrowed: bool) {
        let out = lowercase(Cow::Borrowed(input));
        assert_eq!(out, input.to_lowercase(), "input: {input:?}");
        assert_eq!(
            matches!(out, Cow::Borrowed(_)),
            expect_borrowed,
            "input: {input:?}"
        );
    }

    #[test]
    fn lowercase_borrows_when_trivially_lowercase() {
        assert_lowercase("", true);
        assert_lowercase("my_table_1", true);
        assert_lowercase("schema.table with spaces", true);
    }

    #[test]
    fn lowercase_converts_otherwise() {
        assert_lowercase("My_Table", false);
        // Already lowercase, but non-ASCII is never trivially lowercase.
        assert_lowercase("café_日本語_σς", false);
        assert_lowercase("café_É", false);
        assert_lowercase("ΟΔΟΣ", false);
        assert_lowercase("ǅ", false);
        assert_lowercase("İ", false);
    }

    fn assert_uppercase(input: &str, expect_borrowed: bool) {
        let out = uppercase(Cow::Borrowed(input));
        assert_eq!(out, input.to_uppercase(), "input: {input:?}");
        assert_eq!(
            matches!(out, Cow::Borrowed(_)),
            expect_borrowed,
            "input: {input:?}"
        );
    }

    #[test]
    fn uppercase_borrows_when_trivially_uppercase() {
        assert_uppercase("", true);
        assert_uppercase("MY_TABLE_1", true);
        assert_uppercase("SCHEMA.TABLE WITH SPACES", true);
    }

    #[test]
    fn uppercase_converts_otherwise() {
        assert_uppercase("My_Table", false);
        // Already uppercase, but non-ASCII is never trivially uppercase.
        assert_uppercase("CAFÉ_日本語_ΟΔΟΣ", false);
        assert_uppercase("CAFÉ_é", false);
        // Multi-char uppercase mapping.
        assert_uppercase("straße", false);
        // Titlecase digraph: neither lowercase nor uppercase, but still maps.
        assert_uppercase("ǅ", false);
    }

    #[test]
    fn owned_input_passes_through() {
        let out = uppercase(Cow::Owned("ABC".to_string()));
        assert!(matches!(out, Cow::Owned(ref s) if s == "ABC"));
        let out = lowercase(Cow::Owned("abc".to_string()));
        assert!(matches!(out, Cow::Owned(ref s) if s == "abc"));
    }
}
