use std::env;

pub fn is_update_golden_files_mode() -> bool {
    env::var("GOLDIE_UPDATE").ok().is_some_and(|val| val == "1")
}

pub fn is_continuous_integration_environment() -> bool {
    env::var("CONTINUOUS_INTEGRATION").is_ok()
}

#[macro_export]
macro_rules! assert_contains {
    ($actual:expr, $searched_for:expr $(,)?) => {
        match (&$actual, $searched_for) {
            (actual, searched_for) => {
                assert!(actual.contains(searched_for), "Expected {actual:?} to contain {searched_for:?}");
            }
        }
    };

    ($actual:expr, $searched_for:expr, $($arg:tt)* ) => {
        match (&$actual, $searched_for) {
            (actual, searched_for) => {
                let context = format!($($arg)*);
                assert!(actual.contains(searched_for), "{context}: Expected {actual:?} to contain {searched_for:?}");
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    #[test]
    fn test_assert_contains_positive() {
        assert_contains!("hello world", "hel");
        assert_contains!("hello world", "hello world");
        assert_contains!("hello world", "orld");
        assert_contains!("hello world", "orld", "some context goes {:?}", "here");

        // trailing comma like in a function call
        assert_contains!("hello world", "orld",);
        assert_contains!("hello world", "orld", "some context goes {:?}", "here",);
    }

    #[test]
    #[should_panic(expected = r#"Expected "hello world" to contain "heLlo""#)]
    fn test_assert_contains_case_difference() {
        assert_contains!("hello world", "heLlo");
    }

    #[test]
    #[should_panic(expected = r#"Expected "hello world" to contain "abc""#)]
    fn test_assert_contains_not_found() {
        assert_contains!("hello world", "abc");
    }

    #[test]
    #[should_panic(
        expected = r#"some context goes "here": Expected "hello world" to contain "abc""#
    )]
    fn test_assert_contains_not_found_with_context() {
        assert_contains!("hello world", "abc", "some context goes {:?}", "here");
    }

    #[test]
    fn test_assert_contains_collection() {
        let c = vec!["hel", "o", "world"];
        assert_contains!(c, &"hel");
        assert_contains!(c, &"o");

        let c = vec!["hel".to_string(), "o".to_string(), "world".to_string()];
        assert_contains!(c, &"hel".to_string());
        assert_contains!(c, &"o".to_string());

        let c = HashSet::from(["hel", "o", "world"]);
        assert_contains!(c, "hel");
        assert_contains!(c, "o");

        let c = HashSet::from(["hel".to_string(), "o".to_string(), "world".to_string()]);
        assert_contains!(c, "hel");
        assert_contains!(c, "o");
    }

    #[test]
    #[should_panic(expected = r#"Expected ["hel", "lo", "world"] to contain "hello""#)]
    fn test_assert_contains_collection_not_found() {
        assert_contains!(vec!["hel", "lo", "world"], &"hello");
    }
}
