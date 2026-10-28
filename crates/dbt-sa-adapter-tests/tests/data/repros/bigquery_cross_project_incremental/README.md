Validates that BigQuery incremental models preserve existing rows when the execution project differs from the target project.

The schema-filtered `GetObjects` response in `run_1/recordings.db` is intentionally empty to reproduce the affected driver behavior. Keep that response empty when refreshing the recording so the second run must use the target-qualified metadata fallback before executing the incremental merge.
