//! Masks the values of the `DBT_ENV_SECRET*` environment variables in text that is shown or
//! exported, as dbt Core's `env_secrets` and `scrub_secrets` do.

use std::borrow::Cow;

use dbt_error::FsError;
use dbt_jinja_vars::SECRET_ENV_VAR_PREFIX;

/// The non-blank values of the `DBT_ENV_SECRET*` environment variables, which is what
/// dbt Core's `env_secrets` returns.
pub fn env_secret_values() -> Vec<String> {
    std::env::vars()
        .filter(|(key, value)| key.starts_with(SECRET_ENV_VAR_PREFIX) && !value.trim().is_empty())
        .map(|(_, value)| value)
        .collect()
}

/// Replaces every occurrence of each of `secrets` in `text` with `*****`, as dbt Core's
/// `scrub_secrets` does. Returns `None` when `text` holds none of them.
pub fn scrub_secrets<'a>(text: &'a str, secrets: &[String]) -> Option<Cow<'a, str>> {
    let mut scrubbed = Cow::Borrowed(text);
    for secret in secrets {
        if scrubbed.contains(secret.as_str()) {
            scrubbed = Cow::Owned(scrubbed.replace(secret.as_str(), "*****"));
        }
    }

    match scrubbed {
        Cow::Borrowed(_) => None,
        owned @ Cow::Owned(_) => Some(owned),
    }
}

/// Scrubs `secrets` from the message of `err` and of every error chained to it.
pub fn scrub_secrets_from_error(err: Box<FsError>, secrets: &[String]) -> Box<FsError> {
    rewrite_error_text(err, &|text| {
        scrub_secrets(text, secrets).map(Cow::into_owned)
    })
}

/// Rewrites the message of `err` and of every error chained to it. `rewrite` returns the
/// new text, or `None` to keep it.
pub(crate) fn rewrite_error_text(
    mut err: Box<FsError>,
    rewrite: &dyn Fn(&str) -> Option<String>,
) -> Box<FsError> {
    let next = err.pop_next().map(|next| rewrite_error_text(next, rewrite));
    if let Some(context) = rewrite(&err.context) {
        err.context = context;
    }
    // The message also includes the cause, which can't be edited. If the rewrite applies
    // there, keep the rewritten message and drop the cause.
    if let Some(message) = rewrite(&err.to_string()) {
        // No backtrace: one captured here would point at this function, not at the origin.
        let mut rewritten = FsError::new_no_backtrace(err.code, message);
        rewritten.location = err.location.take();
        rewritten.jinja_frames = std::mem::take(&mut err.jinja_frames);
        *err = rewritten;
    }
    match next {
        Some(next) => Box::new(err.with_chained_errors(next)),
        None => err,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{CodeLocationWithFile, ErrorCode, fs_err};

    fn secrets(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn scrub_secrets_replaces_every_secret_with_asterisks() {
        let scrubbed = scrub_secrets(
            "Failed to get tarball from https://tok-1@example.com/tok-2.tar.gz: tok-1",
            &secrets(&["tok-1", "tok-2"]),
        );
        assert_eq!(
            scrubbed.as_deref(),
            Some("Failed to get tarball from https://*****@example.com/*****.tar.gz: *****")
        );
    }

    #[test]
    fn scrub_secrets_returns_none_without_a_secret() {
        assert_eq!(
            scrub_secrets("Package not found", &secrets(&["tok-1"])),
            None
        );
        assert_eq!(scrub_secrets("Package not found", &[]), None);
    }

    #[test]
    fn scrub_secrets_from_error_scrubs_the_message_and_keeps_the_code() {
        let err = fs_err!(
            ErrorCode::PackageDownloadFailed,
            "Failed to get tarball from https://tok-1@example.com/pkg.tar.gz"
        );
        let scrubbed = scrub_secrets_from_error(err, &secrets(&["tok-1"]));
        assert_eq!(scrubbed.code, ErrorCode::PackageDownloadFailed);
        assert_eq!(
            scrubbed.to_string(),
            "Failed to get tarball from https://*****@example.com/pkg.tar.gz"
        );
    }

    #[test]
    fn scrub_secrets_from_error_scrubs_a_secret_in_the_cause() {
        let cause = std::io::Error::other("could not read https://tok-1@example.com");
        let err = Box::new(FsError::new(ErrorCode::IoError, "Failed to fetch").with_cause(cause));
        assert!(err.to_string().contains("tok-1"), "got: {err}");

        let scrubbed = scrub_secrets_from_error(err, &secrets(&["tok-1"]));
        assert_eq!(scrubbed.code, ErrorCode::IoError);
        assert_eq!(
            scrubbed.to_string(),
            "Failed to fetch: could not read https://*****@example.com"
        );
    }

    #[test]
    fn scrub_secrets_from_error_keeps_the_jinja_frames_when_the_cause_is_dropped() {
        let frames = vec![CodeLocationWithFile::new_with_arc(
            3,
            1,
            20,
            PathBuf::from("macros/fetch.sql"),
        )];
        let cause = std::io::Error::other("could not read https://tok-1@example.com");
        let mut err =
            Box::new(FsError::new(ErrorCode::IoError, "Failed to fetch").with_cause(cause));
        err.jinja_frames = frames.clone();

        let scrubbed = scrub_secrets_from_error(err, &secrets(&["tok-1"]));
        assert_eq!(
            scrubbed.to_string(),
            "Failed to fetch: could not read https://*****@example.com"
        );
        assert_eq!(scrubbed.jinja_frames, frames);
    }

    #[test]
    fn scrub_secrets_from_error_scrubs_chained_errors() {
        let err = fs_err!(ErrorCode::PackageDownloadFailed, "first: tok-1")
            .with_chained_errors(fs_err!(ErrorCode::AuthFailed, "second: tok-1"));
        let mut scrubbed = scrub_secrets_from_error(Box::new(err), &secrets(&["tok-1"]));
        assert_eq!(scrubbed.to_string(), "first: *****");
        let next = scrubbed.pop_next().expect("the chained error is kept");
        assert_eq!(next.code, ErrorCode::AuthFailed);
        assert_eq!(next.to_string(), "second: *****");
    }

    #[test]
    fn scrub_secrets_from_error_leaves_an_error_without_secrets_unchanged() {
        let cause = std::io::Error::other("connection refused");
        let err = Box::new(FsError::new(ErrorCode::IoError, "Failed to fetch").with_cause(cause));
        let scrubbed = scrub_secrets_from_error(err, &secrets(&["tok-1"]));
        assert_eq!(scrubbed.to_string(), "Failed to fetch: connection refused");
        assert!(
            std::error::Error::source(&*scrubbed).is_some(),
            "the cause is kept when it holds no secret"
        );
    }
}
