use crate::proto::v1::public::events::fusion::query::{QueryExecuted, QueryOutcome};

// Display trait is intentionally not implemented to avoid inefficient usage.
// Prefer `[enum].as_ref()` or if you need String use `[enum].as_ref().to_string()`.

impl AsRef<str> for QueryOutcome {
    fn as_ref(&self) -> &str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Success => "success",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}

impl QueryExecuted {
    /// Creates a new `QueryExecuted` event indicating start of execution.
    ///
    /// This is thin wrapper around `new` that avoids the need to pass
    /// `None` for fields that are only known at the end of processing,
    /// and auto assigns the appropriate dbt core event code for start of query execution.
    ///
    /// # Arguments
    /// * `sql` - The SQL query being executed.
    /// * `sql_hash` - A hash of the SQL query.
    /// * `adapter_type` - The type of adapter executing the query.
    /// * `unique_id` - An optional unique identifier for the query.
    /// * `query_description` - An optional description of the query.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        sql: String,
        sql_hash: String,
        adapter_type: String,
        unique_id: Option<String>,
        query_description: Option<String>,
    ) -> Self {
        Self {
            sql,
            sql_hash,
            adapter_type,
            unique_id,
            query_description,
            dbt_core_event_code: "E016".to_string(),
            ..Default::default()
        }
    }
}
