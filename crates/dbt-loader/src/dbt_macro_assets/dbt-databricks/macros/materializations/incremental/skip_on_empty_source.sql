{#-- Only strategies (and schema-change modes) for which an empty source is a no-op may skip;
     replace_where, microbatch, insert_overwrite and merge with not_matched_by_source_action
     can delete or overwrite rows even when the source is empty. --#}
{% macro skip_on_empty_source_is_safe(incremental_strategy, on_schema_change) %}
  {%- set strategy_is_safe = (
        incremental_strategy in ['append', 'delete+insert']
        or (incremental_strategy == 'merge' and not config.get('not_matched_by_source_action'))) -%}
  {{ return(on_schema_change == 'ignore' and strategy_is_safe) }}
{% endmacro %}

{% macro skip_merge_on_empty_source(incremental_strategy, on_schema_change, source_sql, target_relation, existing_relation) %}
  {%- if not (config.get('skip_merge_on_empty_source', False) | as_bool)
        or model['language'] != 'sql'
        or not skip_on_empty_source_is_safe(incremental_strategy, on_schema_change) -%}
    {{ return(false) }}
  {%- endif -%}
  {#-- The newline keeps a trailing `--` comment in the model from swallowing the `)` --#}
  {%- set probe_sql -%}
    select 1 from ({{ source_sql }}
    ) as __dbt_empty_source_check limit 1
  {%- endset -%}
  {%- if run_query(probe_sql) | length > 0 -%}
    {{ return(false) }}
  {%- endif -%}
  {{ log("[skip_merge_on_empty_source] " ~ target_relation ~ ": empty source, skipping incremental run", info=True) }}
  {%- call statement('main') -%}
    select 1 where false
  {%- endcall -%}
  {% do apply_grants(target_relation, config.get('grants'), should_revoke(existing_relation, full_refresh_mode=False)) %}
  {{ return(true) }}
{% endmacro %}
