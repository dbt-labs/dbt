{% macro create_table_at(relation, intermediate_relation, compiled_code) %}
  {% set tags = config.get('databricks_tags') %}
  {% set model_columns = model.get('columns', []) %}
  {% set existing_columns = adapter.get_columns_in_relation(intermediate_relation) %}
  {% set contract_config = config.get('contract') %}
  {% set contract_enforced = contract_config and contract_config.enforced %}
  {% if contract_enforced %}
    {% set model_constraints = model.get('constraints', []) %}
  {% else %}
    {% set model_constraints = [] %}
  {% endif %}
  {% set columns_and_constraints = adapter.parse_columns_and_constraints(existing_columns, model_columns, model_constraints, contract_enforced, model.name) %}
  {% set target_relation = relation.enrich(columns_and_constraints[1]) %}

  {% call statement('main') %}
    {{ get_create_table_sql(target_relation, columns_and_constraints[0], compiled_code) }}
  {% endcall %}

  {{ apply_alter_constraints(target_relation) }}
  {{ apply_tags(target_relation, tags) }}
  {% set column_tags = adapter.get_column_tags_from_model(config.model) %}
  {% if column_tags and column_tags.set_column_tags %}
    {{ apply_column_tags(target_relation, column_tags) }}
  {% endif %}

  {% call statement('merge into target') %}
    insert into {{ target_relation }} by name select * from {{ intermediate_relation }}
  {% endcall %}
{% endmacro %}

{% macro get_create_table_sql(target_relation, columns, compiled_code) %}

  {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

  {%- set contract = config.get('contract') -%}
  {%- set contract_enforced = contract and contract.enforced -%}
  {%- if contract_enforced -%}
    {{ get_assert_columns_equivalent(compiled_code) }}
  {%- endif -%}

  {#-- DIVERGENCE BEGIN: catalogs v2 derives CREATE OR REPLACE support from the resolved catalog relation. --#}
  {%- if dbt_version.startswith('2.') and adapter.behavior.use_catalogs_v2.no_warn -%}
    {%- set use_create_or_replace = catalog_relation.supports_create_or_replace() -%}
  {%- else -%}
    {#-- NOTE: file_format is never 'iceberg' (it resolves to 'parquet'); preserving upstream behavior. --#}
    {%- set use_create_or_replace = catalog_relation.file_format in ('delta', 'iceberg') -%}
  {%- endif -%}
  {#-- DIVERGENCE END --#}
  {%- if use_create_or_replace %}
  create or replace table {{ target_relation.render() }}
  {% else %}
  create table {{ target_relation.render() }}
  {% endif -%}
  {{ get_column_and_constraints_sql(target_relation, columns) }}
  {{ file_format_clause(catalog_relation) }}
  {{ databricks__options_clause(catalog_relation) }}
  {{ partition_cols(label="partitioned by") }}
  {{ get_create_row_filter_clause(target_relation) }}
  {{ liquid_clustered_cols() }}
  {{ clustered_cols(label="clustered by") }}
  {{ location_clause(catalog_relation) }}
  {{ comment_clause() }}
  {{ tblproperties_clause() }}
{% endmacro %}

{% macro databricks__create_table_as(temporary, relation, compiled_code, language='sql') -%}

  {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

  {%- if language == 'sql' -%}
    {%- if temporary -%}
      {{ create_temporary_view(relation, compiled_code) }}
    {%- else -%}
      {#-- DIVERGENCE BEGIN: catalogs v2 derives CREATE OR REPLACE support from the resolved catalog relation. --#}
      {%- if dbt_version.startswith('2.') and adapter.behavior.use_catalogs_v2.no_warn -%}
        {%- set use_create_or_replace = catalog_relation.supports_create_or_replace() -%}
      {%- else -%}
        {#-- NOTE: misses iceberg (file_format resolves to 'parquet'); preserving upstream behavior. --#}
        {%- set use_create_or_replace = catalog_relation.file_format == 'delta' -%}
      {%- endif -%}
      {#-- DIVERGENCE END --#}
      {% if use_create_or_replace %}
        create or replace table {{ relation.render() }}
      {% else %}
        create table {{ relation.render() }}
      {% endif %}
      {%- set contract_config = config.get('contract') -%}
      {% if contract_config and contract_config.enforced %}
        {{ get_assert_columns_equivalent(compiled_code) }}
        {%- set compiled_code = get_select_subquery(compiled_code) %}
      {% endif %}
      {{ file_format_clause(catalog_relation) }}
      {{ databricks__options_clause(catalog_relation) }}
      {{ partition_cols(label="partitioned by") }}
      {{ get_create_row_filter_clause(relation) }}
      {{ liquid_clustered_cols() }}
      {{ clustered_cols(label="clustered by") }}
      {{ location_clause(catalog_relation) }}
      {{ comment_clause() }}
      {{ tblproperties_clause() }}
      as
      {{ compiled_code }}
    {%- endif -%}
  {%- elif language == 'python' -%}
    {#--
    N.B. Python models _can_ write to temp views HOWEVER they use a different session
    and have already expired by the time they need to be used (I.E. in merges for incremental models)

    TODO: Deep dive into spark sessions to see if we can reuse a single session for an entire
    dbt invocation.
     --#}
    {{ databricks__py_write_table(compiled_code=compiled_code, target_relation=relation) }}
  {%- endif -%}
{%- endmacro -%}

{% macro databricks__options_clause(catalog_relation=none) -%}
  {#-
    Moving forward, this macro should require a `catalog_relation`, which is covered by the first condition.
    However, there could be existing macros that is still passing no arguments, including user macros.
    Hence, we need to support the old code still, which is covered by the second condition.
    Additionally, since this rolls up to `options_clause` in `dbt-spark`, which does not have any arguments,
    all calls to `options_clause` will take the second path. This macro needs to be called directly
    via `databricks__options_clause`.
  -#}
  {%- if catalog_relation is not none -%}
    {%- set file_format = catalog_relation.file_format -%}
  {%- else -%}
    {%- set file_format = adapter.resolve_file_format(config) -%}
  {%- endif -%}

  {%- set options = config.get('options') -%}
  {%- if file_format == 'hudi' -%}
    {%- set unique_key = config.get('unique_key') -%}
    {%- if unique_key is not none and options is none -%}
      {%- set options = {'primaryKey': config.get('unique_key')} -%}
    {%- elif unique_key is not none and options is not none and 'primaryKey' not in options -%}
      {%- set _ = options.update({'primaryKey': config.get('unique_key')}) -%}
    {%- elif options is not none and 'primaryKey' in options and options['primaryKey'] != unique_key -%}
      {{ exceptions.raise_compiler_error("unique_key and options('primaryKey') should be the same column(s).") }}
    {%- endif %}
  {%- endif %}

  {%- if options is not none %}
    options (
      {%- for option in options -%}
      {{ option }} "{{ options[option] }}" {% if not loop.last %}, {% endif %}
      {%- endfor %}
    )
  {%- endif %}
{%- endmacro -%}

{% macro get_create_intermediate_table(relation, compiled_code, language) %}
  {%- if language == 'sql' -%}
    {{ create_temporary_view(relation, compiled_code) }}
  {%- else -%}
    {{ create_python_intermediate_table(relation, compiled_code) }}
  {%- endif -%}
{% endmacro %}
