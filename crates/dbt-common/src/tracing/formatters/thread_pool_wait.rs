use super::duration::format_duration_fixed_width;

pub fn format_thread_pool_wait_start() -> String {
    "Started thread pool wait".to_string()
}

pub fn format_thread_pool_wait_end(duration: std::time::Duration) -> String {
    format!(
        "Finished thread pool wait [{}]",
        format_duration_fixed_width(duration)
    )
}
