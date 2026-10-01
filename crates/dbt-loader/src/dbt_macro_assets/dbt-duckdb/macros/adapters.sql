{% macro duckdb__create_schema(relation) -%}
  {%- call statement('create_schema') -%}
    {% set sql %}
        select type from duckdb_databases()
        where lower(database_name)='{{ relation.database | lower }}'
        and type='sqlite'
    {% endset %}
    {% set results = run_query(sql) %}
    {% if results|length == 0 %}
        create schema if not exists {{ relation.without_identifier() }}
    {% else %}
        {% if relation.schema!='main' %}
            {{ exceptions.raise_compiler_error(
                "Schema must be 'main' when writing to sqlite "
                ~ "instead got " ~ relation.schema
            )}}
        {% endif %}
    {% endif %}
  {%- endcall -%}
{% endmacro %}

{% macro duckdb__drop_schema(relation) -%}
  {%- call statement('drop_schema') -%}
    drop schema if exists {{ relation.without_identifier() }} cascade
  {%- endcall -%}
{% endmacro %}

{% macro duckdb__list_schemas(database) -%}
  {% set sql %}
    select schema_name
    from system.information_schema.schemata
    {% if database is not none %}
    where lower(catalog_name) = '{{ database | lower }}'
    {% endif %}
  {% endset %}
  {{ return(run_query(sql)) }}
{% endmacro %}

{% macro duckdb__check_schema_exists(information_schema, schema) -%}
  {% set sql -%}
        select count(*)
        from system.information_schema.schemata
        where lower(schema_name) = '{{ schema | lower }}'
        and lower(catalog_name) = '{{ information_schema.database | lower }}'
  {%- endset %}
  {{ return(run_query(sql)) }}
{% endmacro %}

{% macro get_column_names() %}
  {# loop through user_provided_columns to get column names #}
    {%- set user_provided_columns = model['columns'] -%}
    (
    {% for i in user_provided_columns %}
      {% set col = user_provided_columns[i] %}
      {{ col['name'] }} {{ "," if not loop.last }}
    {% endfor %}
  )
{% endmacro %}


{% macro duckdb__create_table_as(temporary, relation, compiled_code, language='sql') -%}
  {# DIVERGENCE: adapter.build_catalog_relation is Fusion-only (see issue #10659).
     Under dbt-core (1.x) there is no catalog routing, so treat the target as a
     plain create-as-select target (stage-create-capable, non-iceberg). #}
  {%- set write_strategy = adapter.build_catalog_relation(config.model).duckdb_write_strategy if dbt_version.startswith('2.') else 'create_as_select' -%}
  {%- if language == 'sql' -%}
    {% set contract_config = config.get('contract') %}
    {% if contract_config.enforced %}
      {{ get_assert_columns_equivalent(compiled_code) }}
    {% endif %}
    {%- set sql_header = config.get('sql_header', none) -%}

    {{ sql_header if sql_header is not none }}

    {%- if not temporary and write_strategy == 'direct_create' %}
      {% if contract_config.enforced %}
        {#-- DuckDB doesnt support constraints on temp tables --#}
        create table
          {{ relation.include(database=True, schema=True) }}
        {{ get_table_columns_and_constraints() }} ;
        insert into {{ relation }} {{ get_column_names() }} (
          {{ get_select_subquery(compiled_code) }}
        );
      {% else %}
        {%- set columns = get_column_schema_from_query(compiled_code, sql_header) -%}
        {#-- These columns are derived from the SELECT, so they carry no `quote:`
             config of their own. Look the flag up by name in the model's yaml
             `columns:` block, matching `default__get_column_names`; unflagged
             columns are emitted bare rather than quoted unconditionally.
             `or {}` guards the lookups below when `columns:` is absent. --#}
        {%- set user_provided_columns = model['columns'] or {} -%}
        create table
          {{ relation.include(database=True, schema=True) }}
        (
          {% for col in columns -%}
            {%- set col_name = adapter.quote(col.name) if user_provided_columns.get(col.name, {}).get('quote') else col.name -%}
            {{ col_name }} {{ col.dtype }}{{ "," if not loop.last }}
          {% endfor -%}
        );
        insert into {{ relation }} (
          {% for col in columns -%}
            {%- set col_name = adapter.quote(col.name) if user_provided_columns.get(col.name, {}).get('quote') else col.name -%}
            {{ col_name }}{{ "," if not loop.last }}
          {% endfor -%}
        ) (
          {{ compiled_code }}
        );
      {% endif %}
    {%- else %}
    create {% if temporary: -%}temporary{%- endif %} table
      {{ relation.include(database=(not temporary), schema=(not temporary)) }}
  {% if contract_config.enforced and not temporary %}
    {#-- DuckDB doesnt support constraints on temp tables --#}
    {{ get_table_columns_and_constraints() }} ;
    insert into {{ relation }} {{ get_column_names() }} (
      {{ get_select_subquery(compiled_code) }}
    );
  {% else %}
    as (
      {{ compiled_code }}
    );
  {% endif %}
    {% endif %}
  {%- elif language == 'python' -%}
    {% if not temporary and write_strategy != 'create_as_select' %}
      {% do exceptions.raise_compiler_error("DuckDB Python models cannot materialize to an Iceberg catalog that requires create-then-insert writes. Use a SQL model.") %}
    {% endif %}
    {{ py_write_table(temporary=temporary, relation=relation, compiled_code=compiled_code) }}
  {%- else -%}
      {% do exceptions.raise_compiler_error("duckdb__create_table_as macro didn't get supported language, it got %s" % language) %}
  {%- endif -%}
{% endmacro %}

{% macro py_write_table(temporary, relation, compiled_code) -%}
{{ compiled_code }}

def materialize(df, con):
    try:
        import pyarrow
        pyarrow_available = True
    except ImportError:
        pyarrow_available = False
    finally:
        if pyarrow_available and isinstance(df, pyarrow.Table):
            # https://github.com/duckdb/duckdb/issues/6584
            import pyarrow.dataset
    tmp_name = '__dbt_python_model_df_' + '{{ relation.identifier }}'
    con.register(tmp_name, df)
    con.execute('create table {{ relation }} as select * from ' + tmp_name)
    con.unregister(tmp_name)
{% endmacro %}

{% macro duckdb__create_view_as(relation, sql) -%}
  {% set contract_config = config.get('contract') %}
  {% if contract_config.enforced %}
    {{ get_assert_columns_equivalent(sql) }}
  {%- endif %}
  {%- set sql_header = config.get('sql_header', none) -%}

  {{ sql_header if sql_header is not none }}
  create view {{ relation }} as (
    {{ sql }}
  );
{% endmacro %}

{% macro duckdb__get_columns_in_relation(relation) -%}
  {# DIVERGENCE: adapter.table_format is Fusion-only (see issue #10659). Under dbt-core
     (1.x) fmt is none, so we use the standard information_schema.columns query. #}
  {% set fmt = adapter.table_format(relation) if dbt_version.startswith('2.') else none %}
  {% if fmt == 'iceberg' %}
    {# information_schema.columns returns incorrect metadata for Iceberg REST catalog tables. #}
    {% set get_columns_sql %}
      select
          column_name,
          column_type                as data_type,
          null::integer              as character_maximum_length,
          null::integer              as numeric_precision,
          null::integer              as numeric_scale
      from (describe {{ relation }})
    {% endset %}
  {% else %}
    {% set get_columns_sql %}
      select
          column_name,
          data_type,
          character_maximum_length,
          numeric_precision,
          numeric_scale

      from system.information_schema.columns
      where table_name = '{{ relation.identifier }}'
      {% if relation.schema %}
      and lower(table_schema) = '{{ relation.schema | lower }}'
      {% endif %}
      {% if relation.database %}
      and lower(table_catalog) = '{{ relation.database | lower }}'
      {% endif %}
      order by ordinal_position

    {% endset %}
  {% endif %}
  {% call statement('get_columns_in_relation', fetch_result=True) %}
    {{ get_columns_sql }}
  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {{ return(sql_convert_columns_in_relation(table)) }}
{% endmacro %}

{% macro duckdb__list_relations_without_caching(schema_relation) %}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    select
      '{{ schema_relation.database }}' as database,
      table_name as name,
      table_schema as schema,
      CASE table_type
        WHEN 'BASE TABLE' THEN 'table'
        WHEN 'VIEW' THEN 'view'
        WHEN 'LOCAL TEMPORARY' THEN 'table'
        END as type
    from system.information_schema.tables
    where lower(table_schema) = '{{ schema_relation.schema | lower }}'
    and lower(table_catalog) = '{{ schema_relation.database | lower }}'
  {% endcall %}
  {{ return(load_result('list_relations_without_caching').table) }}
{% endmacro %}

{% macro duckdb__drop_relation(relation) -%}
  {# DIVERGENCE: adapter.table_format is Fusion-only (see issue #10659). Under dbt-core
     (1.x) treat as non-iceberg/non-ducklake and always cascade. #}
  {% set fmt = adapter.table_format(relation) if dbt_version.startswith('2.') else none %}
  {% call statement('drop_relation', auto_begin=False) -%}
    {% if fmt == 'ducklake' or fmt == 'iceberg' %}
      drop {{ relation.type }} if exists {{ relation }}
    {% else %}
      drop {{ relation.type }} if exists {{ relation }} cascade
    {% endif %}
  {%- endcall %}
{% endmacro %}

{% macro duckdb__rename_relation(from_relation, to_relation) -%}
  {# DIVERGENCE: adapter.table_format is Fusion-only (see issue #10659). Under dbt-core
     (1.x) fmt is none, so auto_begin stays on and we just ALTER RENAME. #}
  {% set fmt = adapter.table_format(from_relation) if dbt_version.startswith('2.') else none %}
  {% set target_name = adapter.quote_as_configured(to_relation.identifier, 'identifier') %}
  {# Iceberg REST: ALTER...RENAME must not share a transaction with the prior CTAS.
     DuckDB autocommits each statement, so for iceberg we run the rename standalone
     (auto_begin=False) rather than committing the connection — which in Fusion is a
     resource shared across nodes and must not be mutated mid-DAG. #}
  {% call statement('rename_relation', auto_begin=(fmt != 'iceberg')) -%}
    alter {{ to_relation.type }} {{ from_relation }} rename to {{ target_name }}
  {%- endcall %}
{% endmacro %}

{% macro duckdb__make_temp_relation(base_relation, suffix) %}
    {% set suffix = adapter.generate_unique_temporary_table_suffix(suffix) %}
    {% set tmp_identifier = base_relation.identifier ~ suffix %}
    {% do return(base_relation.incorporate(
                                  path={
                                    "identifier": tmp_identifier,
                                    "schema": none,
                                    "database": none
                                  })) -%}
{% endmacro %}

{% macro duckdb__current_timestamp() -%}
  now()
{%- endmacro %}

{% macro duckdb__snapshot_string_as_time(timestamp) -%}
    {%- set result = "'" ~ timestamp ~ "'::timestamp" -%}
    {{ return(result) }}
{%- endmacro %}

{% macro duckdb__snapshot_get_time() -%}
  {{ current_timestamp() }}::timestamp
{%- endmacro %}

{% macro duckdb__get_incremental_default_sql(arg_dict) %}
  {% do return(get_incremental_delete_insert_sql(arg_dict)) %}
{% endmacro %}

{% macro location_exists(location) -%}
  {% do return(adapter.location_exists(location)) %}
{% endmacro %}

{% macro write_to_file(relation, location, options) -%}
  {% call statement('write_to_file') -%}
    copy {{ relation }} to '{{ location }}' ({{ options }})
  {%- endcall %}
{% endmacro %}

{% macro store_relation(plugin, relation, location, format, config) -%}
  {%- set column_list = adapter.get_columns_in_relation(relation) -%}
  {% do adapter.store_relation(plugin, relation, column_list, location, format, config) %}
{% endmacro %}

{% macro render_write_options(config) -%}
  {% set options = config.get('options', {}) %}
  {% if options is not mapping %}
    {% do exceptions.raise_compiler_error("The options argument must be a dictionary") %}
  {% endif %}

  {% for k in options %}
    {% set _ = options.update({k: render(options[k])}) %}
  {% endfor %}

  {# legacy top-level write options #}
  {% if config.get('format') %}
    {% set _ = options.update({'format': render(config.get('format'))}) %}
  {% endif %}
  {% if config.get('delimiter') %}
    {% set _ = options.update({'delimiter': render(config.get('delimiter'))}) %}
  {% endif %}

  {% do return(options) %}
{%- endmacro %}

{% macro duckdb__apply_grants(relation, grant_config, should_revoke=True) %}
    {#-- If grant_config is {} or None, this is a no-op --#}
    {% if grant_config %}
      {{ adapter.warn_once('Grants for relations are not supported by DuckDB') }}
    {% endif %}
{% endmacro %}

{% macro duckdb__get_create_index_sql(relation, index_dict) -%}
  {%- set index_config = adapter.parse_index(index_dict) -%}
  {%- set comma_separated_columns = ", ".join(index_config.columns) -%}
  {%- set index_name = index_config.render(relation) -%}

  create {% if index_config.unique -%}
    unique
  {%- endif %} index
  "{{ index_name }}"
  on {{ relation }}
  ({{ comma_separated_columns }});
{%- endmacro %}

{% macro drop_indexes_on_relation(relation) -%}
  {% call statement('get_indexes_on_relation', fetch_result=True) %}
    SELECT index_name
    FROM duckdb_indexes()
    WHERE schema_name = '{{ relation.schema }}'
      AND table_name = '{{ relation.identifier }}'
  {% endcall %}

  {% set results = load_result('get_indexes_on_relation').table %}
  {% for row in results %}
    {% set index_name = row[0] %}
    {% call statement('drop_index_' + loop.index|string, auto_begin=false) %}
      DROP INDEX "{{ relation.schema }}"."{{ index_name }}"
    {% endcall %}
  {% endfor %}

  {#-- Verify indexes were dropped --#}
  {% call statement('verify_indexes_dropped', fetch_result=True) %}
    SELECT COUNT(*) as remaining_indexes
    FROM duckdb_indexes()
    WHERE schema_name = '{{ relation.schema }}'
      AND table_name = '{{ relation.identifier }}'
  {% endcall %}
  {% set verify_results = load_result('verify_indexes_dropped').table %}
{%- endmacro %}
