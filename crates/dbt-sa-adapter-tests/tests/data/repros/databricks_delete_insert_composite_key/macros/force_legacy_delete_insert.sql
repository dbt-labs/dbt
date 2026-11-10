{#--
  Force the legacy DELETE FROM + INSERT INTO path in
  `dbt-databricks/macros/materializations/incremental/strategies.sql`
  regardless of the warehouse DBR version.

  The fix under test (composite unique_key using correlated EXISTS instead
  of row-valued IN) only fires when `delete_insert_legacy_sql` is chosen.
  On DBR 17.1+ SQL warehouses `delete_insert_sql_impl` normally selects the
  `REPLACE ON` path, so this override skips the capability check and calls
  `delete_insert_legacy_sql` directly.
--#}
{% macro get_delete_insert_sql(arg_dict) -%}
  {%- set source_relation = arg_dict.get('temp_relation') -%}
  {%- set target_relation = arg_dict.get('target_relation') -%}
  {%- set incremental_predicates = config.get('incremental_predicates') -%}
  {%- set target_columns = (adapter.get_columns_in_relation(target_relation) | map(attribute='quoted') | list) -%}
  {%- set unique_key = config.require('unique_key') -%}
  {%- set unique_keys = unique_key if unique_key is sequence and unique_key is not string else [unique_key] -%}
  {% do return(delete_insert_legacy_sql(source_relation, target_relation, target_columns, unique_keys, incremental_predicates)) %}
{%- endmacro %}
