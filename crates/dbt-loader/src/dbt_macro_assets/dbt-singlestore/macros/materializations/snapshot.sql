{#
  SingleStore snapshot materialization support.

  SingleStore does not have a MERGE statement, so the default dbt snapshot
  materialization dispatches to `singlestore__snapshot_merge_sql` which
  implements the merge semantics using two statements:
    1. UPDATE ... JOIN ... SET dbt_valid_to = ...
    2. INSERT INTO ... SELECT ... WHERE dbt_change_type = 'insert'

  The default `materialization snapshot` (from dbt-adapters) works out of the
  box for SingleStore because it calls `snapshot_merge_sql`, which dispatches
  to our `singlestore__snapshot_merge_sql` override below.

  What this file provides:
  - `singlestore__snapshot_hash_arguments`: uses MD5() for SCD id generation
  - `singlestore__post_snapshot`:           drops the staging table after use
#}

{# Use SingleStore's built-in MD5() for snapshot SCD hash generation #}
{% macro singlestore__snapshot_hash_arguments(args) -%}
  md5({%- for arg in args -%}
    coalesce(cast({{ arg }} as char), '')
    {% if not loop.last %} || '|' || {% endif %}
  {%- endfor -%})
{%- endmacro %}

{# Drop the temporary staging table created during snapshot execution #}
{% macro singlestore__post_snapshot(staging_relation) %}
    {{ drop_relation_if_exists(staging_relation) }}
{% endmacro %}
