#[cfg(test)]
mod invocation_id_tests;

// Databricks includes the command invocation ID in its default query comment.
// Keep this adapter-specific so the default comments of other adapters remain unchanged.
pub(super) const DATABRICKS_QUERY_COMMENT: &str = "
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
