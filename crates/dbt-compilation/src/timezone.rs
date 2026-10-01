use dbt_schemas::state::ResolverState;

/// Extracts the configured execution timezone (if any) from the resolver
/// state's profile.
pub fn profile_execution_time_zone(resolver_state: &ResolverState) -> Option<String> {
    resolver_state
        .dbt_profile
        .default_db_config()
        .to_mapping()
        .ok()
        .and_then(|m| {
            m.get("execution_timezone")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        })
}
