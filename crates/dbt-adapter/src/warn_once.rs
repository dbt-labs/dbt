//! Warnings emitted at most once per process, each behind its own [`Once`].

use std::sync::Once;

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;

macro_rules! once_warning {
    ($(#[$doc:meta])* $name:ident, $message:expr) => {
        $(#[$doc])*
        pub fn $name() {
            static ONCE: Once = Once::new();
            ONCE.call_once(|| emit_warn_log_message(ErrorCode::JinjaWarn, $message));
        }
    };
}

/// Run the warning registered for the macro template `<package>.<macro>`.
pub fn warn_once_from_macro(macro_template_name: &str) {
    match macro_template_name {
        "dbt_duckdb.duckdb__apply_grants" => warn_duckdb_grants_unsupported_once(),
        _ => debug_assert!(
            false,
            "no once-only warning registered for macro '{macro_template_name}'"
        ),
    }
}

once_warning!(
    /// `duckdb__apply_grants` skips grants, as DuckDB has none.
    warn_duckdb_grants_unsupported_once,
    "Grants for relations are not supported by DuckDB"
);

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::*;

    struct CountWarnings(Arc<AtomicUsize>);

    impl Subscriber for CountWarnings {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            if *event.metadata().level() == Level::WARN {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    // The flag is process-wide, so this must be the only test that reaches it.
    #[test]
    fn duckdb_grants_warns_once_across_calls() {
        let warnings = Arc::new(AtomicUsize::new(0));
        tracing::subscriber::with_default(CountWarnings(warnings.clone()), || {
            for _ in 0..3 {
                warn_once_from_macro("dbt_duckdb.duckdb__apply_grants");
            }
        });
        assert_eq!(warnings.load(Ordering::Relaxed), 1);
    }
}
