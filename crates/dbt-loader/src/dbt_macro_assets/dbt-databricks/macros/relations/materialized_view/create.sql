{% macro databricks__get_create_materialized_view_as_sql(relation, sql) -%}
  {# Column masks are supported in DBSQL, but not yet wired up to the adapter. Return a helpful error until supported. #}
  {% if column_mask_exists() %}
    {% do exceptions.raise_compiler_error("Column masks are not yet supported for materialized views.") %}
  {% endif %}
  {# DIVERGENCE START: We return the config directly, so we no longer access materialized_view.config. #}
  {%- set materialized_view = adapter.get_config_from_model(config.model) -%}
  {# DIVERGENCE START: partitioned_by is used here instead of partition_by when retrieving partitioning config #}
  {%- set partition_by = materialized_view["partitioned_by"].partition_by -%}
  {# DIVERGENCE END #}
  {%- set tblproperties = materialized_view["tblproperties"].tblproperties -%}
  {%- set comment = materialized_view["comment"].comment -%}
  {%- set refresh = materialized_view["refresh"] -%}
  {# DIVERGENCE END #}

  {#
    TODO: When DESCRIBE QUERY EXTENDED is supported, this implementation should be simplified
    to use that instead. For now, we work around this limitation by writing results to a
    temporary view and using DESCRIBE TABLE EXTENDED on the temporary view.
  #}
  {%- set temp_relation = make_temp_relation(relation) -%}
  {% call statement('create_temp_view') -%}
    {%- set sql_with_limit = sql.rstrip('; \n\t') ~ ' LIMIT 10' -%}
    {{ create_temporary_view(temp_relation, sql_with_limit) }}
  {%- endcall %}

  {%- set columns = adapter.get_columns_in_relation(temp_relation) -%}
  {%- set model_columns = model.get('columns', {}) -%}
  {%- set contract_config = config.get('contract') -%}
  {%- set contract_enforced = contract_config and contract_config.enforced -%}
  {%- if contract_enforced -%}
    {%- do exceptions.warn(
         "contract.enforced=true on materialized_view '" ~ model.name ~ "': not supported by dbt (https://docs.getdbt.com/docs/mesh/govern/model-contracts). dbt-databricks provides best-effort support that may change without notice."
       ) -%}
    {%- set model_constraints = model.get('constraints', []) -%}
  {%- else -%}
    {%- set model_constraints = [] -%}
  {%- endif -%}
  {%- set columns_and_constraints = adapter.parse_columns_and_constraints(columns, model_columns, model_constraints, contract_enforced, model.name) -%}
  {%- set target_relation = relation.enrich(columns_and_constraints[1]) -%}

  create or replace materialized view {{ target_relation.render() }}
    {{ get_column_and_constraints_sql(target_relation, columns_and_constraints[0]) }}
    {{ get_create_row_filter_clause(target_relation) }}
    {{ get_create_sql_partition_by(partition_by) }}
    {{ liquid_clustered_cols() }}
    {{ get_create_sql_comment(comment) }}
    {{ get_create_sql_tblproperties(tblproperties) }}
    {{ get_create_sql_refresh_schedule(refresh) }}
  as
    {{ sql }}
{% endmacro %}
