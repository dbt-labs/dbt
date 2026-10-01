use std::path::Path;

use uuid::Uuid;

/// Emit discrete events during dbt execution.
pub trait DiscreteEventEmitter: Send + Sync {
    fn configure(&mut self, send_anonymous_usage_stats: bool);

    fn dbt_distribution(&self) -> &'static str;

    fn invocation_start_event(
        &self,
        invocation_id: &Uuid,
        root_project_name: &str,
        profile_path: Option<&Path>,
        command: String,
    );
}
