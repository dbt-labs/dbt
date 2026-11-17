use adbc_core::error::{Error, Status};

pub(super) fn refused_publication(error: &Error) -> bool {
    // This driver error guarantees publication was refused. A generic IO
    // error or unknown outcome cannot safely replay a write.
    // https://github.com/redis/redis-adbc-driver/blob/225e00569986e8340acd5978345f3275a889ade6/go/snapshot.go#L1272
    error.status == Status::IO
        && error.sqlstate == b"40001".map(|b| b as std::ffi::c_char)
        && error
            .message
            .contains("snapshot catalog changed concurrently; retry the statement")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retry::QueryRetryPolicy;
    use dbt_adapter_core::AdapterType;
    use dbt_auth::AdapterConfig;
    use dbt_common::Cancellable;

    fn conflict() -> Error {
        let mut error = Error::with_message_and_status(
            "snapshot catalog changed concurrently; retry the statement",
            Status::IO,
        );
        error.sqlstate = b"40001".map(|b| b as std::ffi::c_char);
        error
    }

    #[test]
    fn retries_only_an_explicit_refused_publication() {
        let policy = QueryRetryPolicy::new(AdapterType::RedisAdbc, &AdapterConfig::default());
        let mut calls = 0;
        let result = policy.execute(|| {
            calls += 1;
            if calls == 1 {
                Err(Cancellable::Error(conflict()))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 2);
    }

    #[test]
    fn unknown_outcomes_and_other_statuses_are_not_replayed() {
        let mut unknown = conflict();
        unknown.message = "publication outcome unknown".into();
        assert!(!refused_publication(&unknown));
        let mut wrong_state = conflict();
        wrong_state.sqlstate = b"08006".map(|b| b as std::ffi::c_char);
        assert!(!refused_publication(&wrong_state));
        let mut cancelled = conflict();
        cancelled.status = Status::Cancelled;
        assert!(!refused_publication(&cancelled));
    }
}
