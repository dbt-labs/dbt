use std::fmt::Debug;

use dbt_adapter_core::AdapterType;
use dbt_schemas::schemas::ResolvedCloudConfig;
use dbt_schemas::schemas::project::QueryComment;

use indexmap::IndexMap;
use minijinja::{Error, State};
use regex::Regex;
use serde::Deserialize;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;

// Reference: https://github.com/dbt-labs/dbt-adapters/blob/317e809abd19026d3784e04281b307c5e6a9d469/dbt-adapters/src/dbt/adapters/contracts/connection.py#L197
const DEFAULT_QUERY_COMMENT: &str = "
{%- set comment_dict = {} -%}
{%- do comment_dict.update(
    app='dbt',
    dbt_version=dbt_version,
    profile_name=target.get('profile_name'),
    target_name=target.get('target_name'),
) -%}
{%- if node is not none -%}
  {%- do comment_dict.update(
    node_id=node.unique_id,
  ) -%}
{% else %}
  {# in the node context, the connection name is the node_id #}
  {%- do comment_dict.update(connection_name=connection_name) -%}
{%- endif -%}
{{ return(tojson(comment_dict)) }}
";

// Databricks includes the command invocation ID in its default query comment.
// Keep this adapter-specific so the default comments of other adapters remain unchanged.
const DATABRICKS_QUERY_COMMENT: &str = "
{%- set comment_dict = {} -%}
{%- do comment_dict.update(
    app='dbt',
    dbt_version=dbt_version,
    profile_name=target.get('profile_name'),
    target_name=target.get('target_name'),
    invocation_id=invocation_id,
) -%}
{%- if node is not none -%}
  {%- do comment_dict.update(
    node_id=node.unique_id,
  ) -%}
{% else %}
  {# in the node context, the connection name is the node_id #}
  {%- do comment_dict.update(connection_name=connection_name) -%}
{%- endif -%}
{{ return(tojson(comment_dict)) }}
";

// Extended default query comment that includes dbt Cloud environment variables when present.
// Used automatically when any DBT_CLOUD_* environment variable is set.
const DEFAULT_QUERY_COMMENT_WITH_CLOUD: &str = "
{%- set comment_dict = {} -%}
{%- do comment_dict.update(
    app='dbt',
    dbt_version=dbt_version,
    profile_name=target.get('profile_name'),
    target_name=target.get('target_name'),
) -%}
{%- if node is not none -%}
  {%- do comment_dict.update(
    node_id=node.unique_id,
  ) -%}
{% else %}
  {# in the node context, the connection name is the node_id #}
  {%- do comment_dict.update(connection_name=connection_name) -%}
{%- endif -%}
{%- if invocation_id is not none -%}
  {%- do comment_dict.update(invocation_id=invocation_id) -%}
{%- endif -%}
{%- set cloud_project_id = env_var('DBT_CLOUD_PROJECT_ID', '') -%}
{%- if cloud_project_id -%}
  {%- do comment_dict.update(dbt_cloud_project_id=cloud_project_id) -%}
{%- endif -%}
{%- set cloud_environment_id = env_var('DBT_CLOUD_ENVIRONMENT_ID', '') -%}
{%- if cloud_environment_id -%}
  {%- do comment_dict.update(dbt_cloud_environment_id=cloud_environment_id) -%}
{%- endif -%}
{%- set cloud_job_id = env_var('DBT_CLOUD_JOB_ID', '') -%}
{%- if cloud_job_id -%}
  {%- do comment_dict.update(dbt_cloud_job_id=cloud_job_id) -%}
{%- endif -%}
{%- set cloud_run_id = env_var('DBT_CLOUD_RUN_ID', '') -%}
{%- if cloud_run_id -%}
  {%- do comment_dict.update(dbt_cloud_run_id=cloud_run_id) -%}
{%- endif -%}
{{ return(tojson(comment_dict)) }}
";

#[derive(Debug)]
pub struct QueryCommentConfig {
    /// The (unresolved) query comment
    comment: String,
    /// Append or prepend the comment
    append: bool,
    /// (BigQuery only) Export comment to job labels
    job_label: bool,
}

/// Returns true if a resolved cloud config indicates a dbt Cloud job.
///
/// Checks for full credentials first (local dbt Cloud CLI with dbt_cloud.yml),
/// then falls back to environment_id, which orchestration always sets via
/// DBT_CLOUD_ENVIRONMENT_ID even when token/host are not available.
fn has_cloud_config(cloud_config: Option<&ResolvedCloudConfig>) -> bool {
    cloud_config
        .map(|c| c.credentials.is_some() || c.environment_id.is_some())
        .unwrap_or(false)
}

impl QueryCommentConfig {
    /// Build a comment config from a QueryComment
    pub fn from_query_comment(
        query_comment: Option<QueryComment>,
        adapter_type: AdapterType,
        use_default: bool,
        cloud_config: Option<&ResolvedCloudConfig>,
    ) -> Self {
        #[derive(Debug, Default, Deserialize)]
        struct _QueryCommentConfig {
            comment: Option<String>,
            append: Option<bool>,
            #[serde(default, alias = "job-label")]
            job_label: bool,
        }

        let default_config = _QueryCommentConfig::default();

        let config = match query_comment {
            Some(QueryComment::String(value)) => _QueryCommentConfig {
                comment: Some(value),
                ..default_config
            },
            Some(QueryComment::Object(value)) => {
                _QueryCommentConfig::deserialize(value).unwrap_or(default_config)
            }
            None => default_config,
        };

        QueryCommentConfig {
            comment: config.comment.unwrap_or_else(|| {
                if use_default {
                    if has_cloud_config(cloud_config) {
                        DEFAULT_QUERY_COMMENT_WITH_CLOUD.to_string()
                    } else {
                        match adapter_type {
                            AdapterType::Databricks => DATABRICKS_QUERY_COMMENT,
                            _ => DEFAULT_QUERY_COMMENT,
                        }
                        .to_string()
                    }
                } else {
                    "".to_string()
                }
            }),
            append: config
                .append
                .unwrap_or(adapter_type == AdapterType::Snowflake),
            job_label: config.job_label && adapter_type == AdapterType::Bigquery,
        }
    }

    /// Resolve query comment given current Jinja state.
    pub fn resolve_comment(&self, state: &State) -> Result<String, Error> {
        state
            .env()
            .render_str(self.comment.as_str(), state.get_base_context(), &[])
    }

    /// Add query comment to SQL.
    pub fn add_comment(&self, sql: &str, resolved_comment: &str) -> String {
        if resolved_comment.is_empty() {
            return sql.to_owned();
        }

        if self.append {
            format!("{sql}\n/* {resolved_comment} */")
        } else {
            format!("/* {resolved_comment} */\n{sql}")
        }
    }

    /// Reference: https://github.com/dbt-labs/dbt-adapters/blob/b0223a88d67012bcc4c6cce5449c4fe10c6ed198/dbt-bigquery/src/dbt/adapters/bigquery/connections.py#L629
    pub fn get_job_labels_from_query_comment(
        &self,
        resolved_comment: &str,
    ) -> IndexMap<String, String> {
        if !self.job_label {
            return IndexMap::new();
        }

        let job_labels = match serde_json::from_str::<JsonMap<String, JsonValue>>(resolved_comment)
        {
            Ok(json) => json
                .into_iter()
                .map(|(key, value)| {
                    let value_str = match value.as_str() {
                        Some(s) => s.to_string(),
                        None => value.to_string(),
                    };
                    (sanitize_label(&key), sanitize_label(&value_str))
                })
                .collect(),
            Err(_) => vec![(
                "query_comment".to_string(),
                sanitize_label(resolved_comment),
            )],
        };

        IndexMap::from_iter(job_labels)
    }
}

const _SANITIZE_LABEL_PATTERN: &str = r"[^a-z0-9_-]";

const _VALIDATE_LABEL_LENGTH_LIMIT: usize = 63;

/// Reference: https://github.com/dbt-labs/dbt-adapters/blob/b0223a88d67012bcc4c6cce5449c4fe10c6ed198/dbt-bigquery/src/dbt/adapters/bigquery/connections.py#L640
fn sanitize_label(label: &str) -> String {
    let value = label.to_lowercase();

    let re = Regex::new(_SANITIZE_LABEL_PATTERN).unwrap();
    let sanitized_value = re.replace_all(&value, "_");

    if sanitized_value.len() > _VALIDATE_LABEL_LENGTH_LIMIT {
        sanitized_value[.._VALIDATE_LABEL_LENGTH_LIMIT].to_string()
    } else {
        sanitized_value.to_string()
    }
}

#[cfg(test)]
#[path = "query_comment/databricks_invocation_id_tests.rs"]
mod databricks_invocation_id_tests;

#[cfg(test)]
mod tests {
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::schemas::ResolvedCloudConfig;
    use dbt_schemas::schemas::project::QueryComment;
    use serde::Deserialize;

    use crate::engine::query_comment::{
        DATABRICKS_QUERY_COMMENT, DEFAULT_QUERY_COMMENT, DEFAULT_QUERY_COMMENT_WITH_CLOUD,
        QueryCommentConfig,
    };

    // Env var tests mutate process-wide state, so they must not run in parallel.

    fn assert_configs_equal(left: &QueryCommentConfig, right: &QueryCommentConfig) {
        assert_eq!(left.comment, right.comment);
        assert_eq!(left.append, right.append);
        assert_eq!(left.job_label, right.job_label);
    }

    #[test]
    fn test_empty_query_comment() {
        // Test empty query comment with `use_default`
        for adapter_type in [AdapterType::Bigquery, AdapterType::Redshift] {
            let config = QueryCommentConfig::from_query_comment(None, adapter_type, true, None);
            let expected_config = QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT.to_string(),
                append: false,
                job_label: false,
            };
            assert_configs_equal(&config, &expected_config);
        }

        let config =
            QueryCommentConfig::from_query_comment(None, AdapterType::Databricks, true, None);
        let expected_config = QueryCommentConfig {
            comment: DATABRICKS_QUERY_COMMENT.to_string(),
            append: false,
            job_label: false,
        };
        assert_configs_equal(&config, &expected_config);

        let config =
            QueryCommentConfig::from_query_comment(None, AdapterType::Snowflake, true, None);
        let expected_config = QueryCommentConfig {
            comment: DEFAULT_QUERY_COMMENT.to_string(),
            append: true,
            job_label: false,
        };
        assert_configs_equal(&config, &expected_config);

        // Test empty query comment with NO `use_default`
        for adapter_type in [
            AdapterType::Bigquery,
            AdapterType::Databricks,
            AdapterType::Redshift,
        ] {
            let config = QueryCommentConfig::from_query_comment(None, adapter_type, false, None);
            let expected_config = QueryCommentConfig {
                comment: "".to_string(),
                append: false,
                job_label: false,
            };
            assert_configs_equal(&config, &expected_config);
        }

        let config =
            QueryCommentConfig::from_query_comment(None, AdapterType::Snowflake, false, None);
        let expected_config = QueryCommentConfig {
            comment: "".to_string(),
            append: true,
            job_label: false,
        };
        assert_configs_equal(&config, &expected_config);
    }

    #[test]
    fn test_query_comment_string() {
        for adapter_type in [
            AdapterType::Bigquery,
            AdapterType::Databricks,
            AdapterType::Redshift,
        ] {
            let config = QueryCommentConfig::from_query_comment(
                Some(QueryComment::String("cool comment".to_string())),
                adapter_type,
                true,
                None,
            );
            let expected_config = QueryCommentConfig {
                comment: "cool comment".to_string(),
                append: false,
                job_label: false,
            };
            assert_configs_equal(&config, &expected_config);

            // Whether `use_default` is set should make no difference now
            let config = QueryCommentConfig::from_query_comment(
                Some(QueryComment::String("cool comment".to_string())),
                adapter_type,
                false,
                None,
            );
            assert_configs_equal(&config, &expected_config);
        }

        let config = QueryCommentConfig::from_query_comment(
            Some(QueryComment::String("cool comment".to_string())),
            AdapterType::Snowflake,
            true,
            None,
        );
        let expected_config = QueryCommentConfig {
            comment: "cool comment".to_string(),
            append: true,
            job_label: false,
        };
        assert_configs_equal(&config, &expected_config);

        let config = QueryCommentConfig::from_query_comment(
            Some(QueryComment::String("cool comment".to_string())),
            AdapterType::Snowflake,
            false,
            None,
        );
        assert_configs_equal(&config, &expected_config);
    }

    #[test]
    fn test_query_comment_object_comment() {
        let config_str = "comment: \"cool comment\"";
        let query_comment =
            QueryComment::deserialize(dbt_yaml::Deserializer::from_str(config_str)).ok();

        for adapter_type in [
            AdapterType::Bigquery,
            AdapterType::Databricks,
            AdapterType::Redshift,
        ] {
            let config = QueryCommentConfig::from_query_comment(
                query_comment.clone(),
                adapter_type,
                true,
                None,
            );
            let expected_config = QueryCommentConfig {
                comment: "cool comment".to_string(),
                append: false,
                job_label: false,
            };
            assert_configs_equal(&config, &expected_config);

            // Whether `use_default` is set should make no difference now
            let config = QueryCommentConfig::from_query_comment(
                query_comment.clone(),
                adapter_type,
                false,
                None,
            );
            assert_configs_equal(&config, &expected_config);
        }

        let config = QueryCommentConfig::from_query_comment(
            query_comment.clone(),
            AdapterType::Snowflake,
            true,
            None,
        );
        let expected_config = QueryCommentConfig {
            comment: "cool comment".to_string(),
            append: true,
            job_label: false,
        };
        assert_configs_equal(&config, &expected_config);

        let config = QueryCommentConfig::from_query_comment(
            query_comment,
            AdapterType::Snowflake,
            false,
            None,
        );
        assert_configs_equal(&config, &expected_config);
    }

    #[test]
    fn test_query_comment_object_append() {
        let config_str = "append: true";
        let query_comment =
            QueryComment::deserialize(dbt_yaml::Deserializer::from_str(config_str)).ok();

        for adapter_type in [
            AdapterType::Bigquery,
            AdapterType::Redshift,
            AdapterType::Snowflake,
        ] {
            let config = QueryCommentConfig::from_query_comment(
                query_comment.clone(),
                adapter_type,
                true,
                None,
            );
            let expected_config = QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT.to_string(),
                append: true,
                job_label: false,
            };
            assert_configs_equal(&config, &expected_config);
        }

        let config = QueryCommentConfig::from_query_comment(
            query_comment,
            AdapterType::Databricks,
            true,
            None,
        );
        let expected_config = QueryCommentConfig {
            comment: DATABRICKS_QUERY_COMMENT.to_string(),
            append: true,
            job_label: false,
        };
        assert_configs_equal(&config, &expected_config);

        let config_str = "append: false";
        let query_comment =
            QueryComment::deserialize(dbt_yaml::Deserializer::from_str(config_str)).ok();

        for adapter_type in [
            AdapterType::Bigquery,
            AdapterType::Databricks,
            AdapterType::Redshift,
            AdapterType::Snowflake,
        ] {
            let config = QueryCommentConfig::from_query_comment(
                query_comment.clone(),
                adapter_type,
                false,
                None,
            );
            let expected_config = QueryCommentConfig {
                comment: "".to_string(),
                append: false,
                job_label: false,
            };
            assert_configs_equal(&config, &expected_config);
        }
    }

    #[test]
    fn test_query_comment_object_job_label() {
        let config_str = "job-label: true";
        let query_comment =
            QueryComment::deserialize(dbt_yaml::Deserializer::from_str(config_str)).ok();

        // This should only work for BigQuery
        for adapter_type in [AdapterType::Redshift, AdapterType::Snowflake] {
            let config = QueryCommentConfig::from_query_comment(
                query_comment.clone(),
                adapter_type,
                true,
                None,
            );
            let expected_config = QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT.to_string(),
                append: adapter_type == AdapterType::Snowflake,
                job_label: false,
            };
            assert_configs_equal(&config, &expected_config);
        }

        let config = QueryCommentConfig::from_query_comment(
            query_comment.clone(),
            AdapterType::Databricks,
            true,
            None,
        );
        let expected_config = QueryCommentConfig {
            comment: DATABRICKS_QUERY_COMMENT.to_string(),
            append: false,
            job_label: false,
        };
        assert_configs_equal(&config, &expected_config);

        let config = QueryCommentConfig::from_query_comment(
            query_comment,
            AdapterType::Bigquery,
            true,
            None,
        );
        let expected_config = QueryCommentConfig {
            comment: DEFAULT_QUERY_COMMENT.to_string(),
            append: false,
            job_label: true,
        };
        assert_configs_equal(&config, &expected_config);
    }

    #[test]
    fn test_cloud_query_comment_with_cloud_config() {
        use dbt_schemas::schemas::CloudCredentials;
        // Verify the cloud template is selected when cloud config has credentials
        let cloud_config = ResolvedCloudConfig {
            credentials: Some(CloudCredentials {
                account_id: "111".to_string(),
                host: "cloud.getdbt.com".to_string(),
                token: "tok".to_string(),
            }),
            ..Default::default()
        };
        let config = QueryCommentConfig::from_query_comment(
            None,
            AdapterType::Bigquery,
            true,
            Some(&cloud_config),
        );
        assert_configs_equal(
            &config,
            &QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT_WITH_CLOUD.to_string(),
                append: false,
                job_label: false,
            },
        );
    }

    #[test]
    fn test_cloud_query_comment_without_cloud_config() {
        let config =
            QueryCommentConfig::from_query_comment(None, AdapterType::Bigquery, true, None);
        assert_configs_equal(
            &config,
            &QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT.to_string(),
                append: false,
                job_label: false,
            },
        );
    }

    #[test]
    fn test_cloud_query_comment_not_used_when_user_provides_comment() {
        // Even with cloud config present, user-provided comments take precedence
        let cloud_config = ResolvedCloudConfig::default();
        let config = QueryCommentConfig::from_query_comment(
            Some(QueryComment::String("user comment".to_string())),
            AdapterType::Bigquery,
            true,
            Some(&cloud_config),
        );
        assert_configs_equal(
            &config,
            &QueryCommentConfig {
                comment: "user comment".to_string(),
                append: false,
                job_label: false,
            },
        );
    }

    #[test]
    fn test_cloud_query_comment_no_config_uses_default() {
        // No cloud config should not trigger the cloud template
        let config =
            QueryCommentConfig::from_query_comment(None, AdapterType::Bigquery, true, None);
        assert_configs_equal(
            &config,
            &QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT.to_string(),
                append: false,
                job_label: false,
            },
        );
    }

    #[test]
    fn test_cloud_query_comment_with_environment_id_only() {
        // Orchestration sets DBT_CLOUD_ENVIRONMENT_ID but not token/host, so
        // credentials is always None in production cloud jobs. environment_id
        // is the reliable signal that we're running as a cloud job.
        let cloud_config = ResolvedCloudConfig {
            environment_id: Some("411414".to_string()),
            ..Default::default()
        };
        for adapter_type in [
            AdapterType::Databricks,
            AdapterType::Redshift,
            AdapterType::Snowflake,
            AdapterType::Bigquery,
        ] {
            let config = QueryCommentConfig::from_query_comment(
                None,
                adapter_type,
                true,
                Some(&cloud_config),
            );
            assert_eq!(
                config.comment, DEFAULT_QUERY_COMMENT_WITH_CLOUD,
                "{adapter_type:?} should use cloud template when environment_id is set",
            );
        }
    }

    #[test]
    fn test_cloud_query_comment_redshift_with_cloud_config() {
        use dbt_schemas::schemas::CloudCredentials;
        // Redshift is the primary adapter for cloud query ID resolution via SYS_QUERY_HISTORY.
        // Verify it selects the cloud template when credentials are present.
        let cloud_config = ResolvedCloudConfig {
            credentials: Some(CloudCredentials {
                account_id: "123".to_string(),
                host: "cloud.getdbt.com".to_string(),
                token: "tok".to_string(),
            }),
            ..Default::default()
        };
        let config = QueryCommentConfig::from_query_comment(
            None,
            AdapterType::Redshift,
            true,
            Some(&cloud_config),
        );
        assert_configs_equal(
            &config,
            &QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT_WITH_CLOUD.to_string(),
                append: false,
                job_label: false,
            },
        );
    }

    #[test]
    fn test_cloud_query_comment_redshift_without_cloud_config_uses_default() {
        // Without cloud config, Redshift must use DEFAULT_QUERY_COMMENT (no cloud fields).
        // This is the pre-fix behaviour that caused zero rows in SYS_QUERY_HISTORY.
        let config =
            QueryCommentConfig::from_query_comment(None, AdapterType::Redshift, true, None);
        assert_configs_equal(
            &config,
            &QueryCommentConfig {
                comment: DEFAULT_QUERY_COMMENT.to_string(),
                append: false,
                job_label: false,
            },
        );
    }

    #[test]
    fn test_cloud_query_comment_includes_invocation_id() {
        assert!(
            DEFAULT_QUERY_COMMENT_WITH_CLOUD.contains("invocation_id"),
            "Cloud query comment template should include invocation_id"
        );
        assert!(
            !DEFAULT_QUERY_COMMENT.contains("invocation_id"),
            "Default query comment template should not include invocation_id"
        );
    }
}
