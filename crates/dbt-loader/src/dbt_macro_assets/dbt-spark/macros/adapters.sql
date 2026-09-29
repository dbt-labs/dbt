-- funcsign: () -> string
{% macro tblproperties_clause() %}
  {{ return(adapter.dispatch('tblproperties_clause', 'dbt')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro spark__tblproperties_clause() -%}
  {%- set tblproperties = config.get('tblproperties') -%}
  {%- if tblproperties is not none %}
    tblproperties (
      {%- for prop in tblproperties -%}
      '{{ prop }}' = '{{ tblproperties[prop] }}' {% if not loop.last %}, {% endif %}
      {%- endfor %}
    )
  {%- endif %}
{%- endmacro -%}

-- funcsign: () -> string
{% macro file_format_clause() %}
  {{ return(adapter.dispatch('file_format_clause', 'dbt')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro spark__file_format_clause() %}
  {%- set file_format = config.get('file_format', validator=validation.any[basestring]) -%}
  {%- if file_format is not none %}
    using {{ file_format }}
  {%- endif %}
{%- endmacro -%}

-- funcsign: () -> string
{% macro location_clause() %}
  {{ return(adapter.dispatch('location_clause', 'dbt')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro spark__location_clause() %}
  {%- set location_root = config.get('location_root', validator=validation.any[basestring]) -%}
  {%- set identifier = model['alias'] -%}
  {%- if location_root is not none %}
    location '{{ location_root }}/{{ identifier }}'
  {%- endif %}
{%- endmacro -%}

-- funcsign: () -> string
{% macro options_clause() -%}
  {{ return(adapter.dispatch('options_clause', 'dbt')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro spark__options_clause() -%}
  {%- set options = config.get('options') -%}
  {%- if config.get('file_format') == 'hudi' -%}
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

-- funcsign: () -> string
{% macro comment_clause() %}
  {{ return(adapter.dispatch('comment_clause', 'dbt')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro spark__comment_clause() %}
  {%- set raw_persist_docs = config.get('persist_docs', {}) -%}

  {%- if raw_persist_docs is mapping -%}
    {%- set raw_relation = raw_persist_docs.get('relation', false) -%}
      {%- if raw_relation -%}
      comment '{{ model.description | replace("'", "\\'") }}'
      {% endif %}
  {%- elif raw_persist_docs -%}
    {{ exceptions.raise_compiler_error("Invalid value provided for 'persist_docs'. Expected dict but got value: " ~ raw_persist_docs) }}
  {% endif %}
{%- endmacro -%}

-- funcsign: (string, bool) -> string
{% macro partition_cols(label, required=false) %}
  {{ return(adapter.dispatch('partition_cols', 'dbt')(label, required)) }}
{%- endmacro -%}

-- funcsign: (string, bool) -> string
{% macro spark__partition_cols(label, required=false) %}
  {%- set cols = config.get('partition_by', validator=validation.any[list, basestring]) -%}
  {%- if cols is not none %}
    {%- if cols is string -%}
      {%- set cols = [cols] -%}
    {%- endif -%}
    {{ label }} (
    {%- for item in cols -%}
      {{ item }}
      {%- if not loop.last -%},{%- endif -%}
    {%- endfor -%}
    )
  {%- endif %}
{%- endmacro -%}

-- funcsign: (string, bool) -> string
{% macro clustered_cols(label, required=false) %}
  {{ return(adapter.dispatch('clustered_cols', 'dbt')(label, required)) }}
{%- endmacro -%}

-- funcsign: (string, bool) -> string
{% macro spark__clustered_cols(label, required=false) %}
  {%- set cols = config.get('clustered_by', validator=validation.any[list, basestring]) -%}
  {%- set buckets = config.get('buckets', validator=validation.any[int]) -%}
  {%- if (cols is not none) and (buckets is not none) %}
    {%- if cols is string -%}
      {%- set cols = [cols] -%}
    {%- endif -%}
    {{ label }} (
    {%- for item in cols -%}
      {{ item }}
      {%- if not loop.last -%},{%- endif -%}
    {%- endfor -%}
    ) into {{ buckets }} buckets
  {%- endif %}
{%- endmacro -%}

{% macro fetch_tbl_properties(relation) -%}
  {% call statement('list_properties', fetch_result=True) -%}
    SHOW TBLPROPERTIES {{ relation }}
  {% endcall %}
  {% do return(load_result('list_properties').table) %}
{%- endmacro %}

-- funcsign: (relation, string) -> string
{% macro create_temporary_view(relation, compiled_code) -%}
  {{ return(adapter.dispatch('create_temporary_view', 'dbt')(relation, compiled_code)) }}
{%- endmacro -%}

{#-- We can't use temporary tables with `create ... as ()` syntax --#}
-- funcsign: (relation, string) -> string
{% macro spark__create_temporary_view(relation, compiled_code) -%}
    create or replace temporary view {{ relation }} as
      {{ compiled_code }}
{%- endmacro -%}


{%- macro spark__create_table_as(temporary, relation, compiled_code, language='sql') -%}
  {%- if language == 'sql' -%}
    {%- if temporary -%}
      {{ create_temporary_view(relation, compiled_code) }}
    {%- else -%}
      {% if config.get('file_format', validator=validation.any[basestring]) in ['delta', 'iceberg'] %}
        create or replace table {{ relation }}
      {% else %}
        create table {{ relation }}
      {% endif %}
      {%- set contract_config = config.get('contract') -%}
      {%- if contract_config.enforced -%}
        {{ get_assert_columns_equivalent(compiled_code) }}
        {%- set compiled_code = get_select_subquery(compiled_code) %}
      {% endif %}
      {{ file_format_clause() }}
      {{ options_clause() }}
      {{ tblproperties_clause() }}
      {{ partition_cols(label="partitioned by") }}
      {{ clustered_cols(label="clustered by") }}
      {{ location_clause() }}
      {{ comment_clause() }}

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
    {{ py_write_table(compiled_code=compiled_code, target_relation=relation) }}
  {%- endif -%}
{%- endmacro -%}

-- funcsign: (relation, string) -> string
{% macro persist_constraints(relation, model) %}
  {{ return(adapter.dispatch('persist_constraints', 'dbt')(relation, model)) }}
{% endmacro %}

-- funcsign: (relation, model) -> string
{% macro spark__persist_constraints(relation, model) %}
  {%- set contract_config = config.get('contract') -%}
  {% if contract_config.enforced and config.get('file_format', 'delta') == 'delta' %}
    {% do alter_table_add_constraints(relation, model.constraints) %}
    {% do alter_column_set_constraints(relation, model.columns) %}
  {% endif %}
{% endmacro %}

{% macro alter_table_add_constraints(relation, constraints) %}
  {{ return(adapter.dispatch('alter_table_add_constraints', 'dbt')(relation, constraints)) }}
{% endmacro %}

{% macro spark__alter_table_add_constraints(relation, constraints) %}
  {% for constraint in constraints %}
    {% if constraint.type == 'check' and not is_incremental() %}
      {%- set constraint_hash = local_md5(column_name ~ ";" ~ constraint.expression ~ ";" ~ loop.index) -%}
      {% call statement() %}
        alter table {{ relation }} add constraint {{ constraint.name if constraint.name else constraint_hash }} check ({{ constraint.expression }});
      {% endcall %}
    {% endif %}
  {% endfor %}
{% endmacro %}

{% macro alter_column_set_constraints(relation, column_dict) %}
  {{ return(adapter.dispatch('alter_column_set_constraints', 'dbt')(relation, column_dict)) }}
{% endmacro %}

{% macro spark__alter_column_set_constraints(relation, column_dict) %}
  {% for column_name in column_dict %}
    {% set constraints = column_dict[column_name]['constraints'] %}
    {% for constraint in constraints %}
      {% if constraint.type != 'not_null' %}
        {{ exceptions.warn('Invalid constraint for column ' ~ column_name ~ '. Only `not_null` is supported.') }}
      {% else %}
        {% set quoted_name = adapter.quote(column_name) if column_dict[column_name]['quote'] else column_name %}
        {% call statement() %}
          alter table {{ relation }} change column {{ quoted_name }} set not null {{ constraint.expression or "" }};
        {% endcall %}
      {% endif %}
    {% endfor %}
  {% endfor %}
{% endmacro %}

{% macro get_column_comment_sql(column_name, column_dict) -%}
  {% if column_name in column_dict and column_dict[column_name]["description"] -%}
    {% set escaped_description = column_dict[column_name]["description"] | replace("'", "\\'") %}
    {% set column_comment_clause = "comment '" ~ escaped_description ~ "'" %}
  {%- endif -%}
  {{ adapter.quote(column_name) }} {{ column_comment_clause }}
{% endmacro %}

{% macro get_persist_docs_column_list(model_columns, query_columns) %}
  {% for column_name in query_columns %}
    {{ get_column_comment_sql(column_name, model_columns) }}
    {{- ", " if not loop.last else "" }}
  {% endfor %}
{% endmacro %}

{% macro spark__create_view_as(relation, sql) -%}
  create or replace view {{ relation }}
  {% if config.persist_column_docs() -%}
    {% set model_columns = model.columns %}
    {% set query_columns = get_columns_in_query(sql) %}
    (
    {{ get_persist_docs_column_list(model_columns, query_columns) }}
    )
  {% endif %}
  {{ comment_clause() }}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{ get_assert_columns_equivalent(sql) }}
  {%- endif %}
  as
    {{ sql }}
{% endmacro %}

{% macro spark__create_schema(relation) -%}
  {%- call statement('create_schema') -%}
    create schema if not exists {{ relation }}
  {% endcall %}
{% endmacro %}

{% macro spark__drop_schema(relation) -%}
  {%- call statement('drop_schema') -%}
    drop schema if exists {{ relation }} cascade
  {%- endcall -%}
{% endmacro %}

{% macro get_columns_in_relation_raw(relation) -%}
  {{ return(adapter.dispatch('get_columns_in_relation_raw', 'dbt')(relation)) }}
{%- endmacro -%}

{% macro spark__get_columns_in_relation_raw(relation) -%}
  {% call statement('get_columns_in_relation_raw', fetch_result=True) %}
      describe extended {{ relation }}
  {% endcall %}
  {% do return(load_result('get_columns_in_relation_raw').table) %}
{% endmacro %}

{% macro spark__get_columns_in_relation(relation) -%}
  {% call statement('get_columns_in_relation', fetch_result=True) %}
      describe extended {{ relation.render() }}
  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {#- DIVERGENCE BEGIN: dbt-core Spark does not cast table to Column. All other adapters do it.
      Gate on dbt_version: under dbt-core (1.x) return the raw table to match upstream behavior. -#}
  {% if dbt_version.startswith('2.') %}
    {% do return(sql_convert_columns_in_relation(table)) %}
  {% else %}
    {% do return(table) %}
  {% endif %}
  {#- DIVERGENCE END -#}
{% endmacro %}

{% macro spark__list_relations_without_caching(relation) %}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    show table extended in {{ relation.schema }} like '*'
  {% endcall %}

  {% do return(load_result('list_relations_without_caching').table) %}
{% endmacro %}

{% macro list_relations_show_tables_without_caching(schema_relation) %}
  {#-- Spark with iceberg tables don't work with show table extended for #}
  {#-- V2 iceberg tables #}
  {#-- https://issues.apache.org/jira/browse/SPARK-33393 #}
  {% call statement('list_relations_without_caching_show_tables', fetch_result=True) -%}
    show tables in {{ schema_relation.schema }} like '*'
  {% endcall %}

  {% do return(load_result('list_relations_without_caching_show_tables').table) %}
{% endmacro %}

{% macro describe_table_extended_without_caching(table_name) %}
  {#-- Spark with iceberg tables don't work with show table extended for #}
  {#-- V2 iceberg tables #}
  {#-- https://issues.apache.org/jira/browse/SPARK-33393 #}
  {% call statement('describe_table_extended_without_caching', fetch_result=True) -%}
    describe extended {{ table_name }}
  {% endcall %}
  {% do return(load_result('describe_table_extended_without_caching').table) %}
{% endmacro %}

{% macro spark__list_schemas(database) -%}
  {% call statement('list_schemas', fetch_result=True, auto_begin=False) %}
    show databases
  {% endcall %}
  {{ return(load_result('list_schemas').table) }}
{% endmacro %}

{% macro spark__check_schema_exists_like(schema) -%}
  {% call statement('check_schema_exists', fetch_result=True, auto_begin=False) %}
    show databases like '{{ schema }}'
  {% endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{% endmacro %}

{% macro spark__rename_relation(from_relation, to_relation) -%}
  {% call statement('rename_relation') -%}
    {% if not from_relation.type %}
      {% do exceptions.raise_database_error("Cannot rename a relation with a blank type: " ~ from_relation.identifier) %}
    {% elif from_relation.type in ('table') %}
        alter table {{ from_relation }} rename to {{ to_relation }}
    {% elif from_relation.type == 'view' %}
        alter view {{ from_relation }} rename to {{ to_relation }}
    {% else %}
      {% do exceptions.raise_database_error("Unknown type '" ~ from_relation.type ~ "' for relation: " ~ from_relation.identifier) %}
    {% endif %}
  {%- endcall %}
{% endmacro %}

{% macro spark__drop_relation(relation) -%}
  {% call statement('drop_relation', auto_begin=False) -%}
    drop {{ relation.type }} if exists {{ relation }}
  {%- endcall %}
{% endmacro %}


{% macro spark__generate_database_name(custom_database_name=none, node=none) -%}
  {% do return(None) %}
{%- endmacro %}

{% macro spark__persist_docs(relation, model, for_relation, for_columns) -%}
  {% if for_columns and config.persist_column_docs() and model.columns %}
    {% set existing_columns = adapter.get_columns_in_relation(relation) | map(attribute="name") | list %}
    {% set filtered_columns = validate_doc_columns(relation, model.columns, existing_columns) %}
    {% do alter_column_comment(relation, filtered_columns) %}
  {% endif %}
{% endmacro %}

{% macro spark__alter_column_comment(relation, column_dict) %}
  {% if config.get('file_format', validator=validation.any[basestring]) in ['delta', 'hudi', 'iceberg'] %}
    {% for column_name in column_dict %}
      {% set comment = column_dict[column_name]['description'] %}
      {% set escaped_comment = comment | replace('\'', '\\\'') %}
      {% set comment_query %}
        {% if relation.is_iceberg %}
          alter table {{ relation }} alter column
              {{ adapter.quote(column_name) if column_dict[column_name]['quote'] else column_name }}
              comment '{{ escaped_comment }}';
        {% else %}
          alter table {{ relation }} change column
              {{ adapter.quote(column_name) if column_dict[column_name]['quote'] else column_name }}
              comment '{{ escaped_comment }}';
        {% endif %}
      {% endset %}
      {% do run_query(comment_query) %}
    {% endfor %}
  {% endif %}
{% endmacro %}


{% macro spark__make_temp_relation(base_relation, suffix) %}
    {% set tmp_identifier = base_relation.identifier ~ suffix %}
    {% set tmp_relation = base_relation.incorporate(path = {
        "identifier": tmp_identifier
    }) -%}

    {%- set tmp_relation = tmp_relation.include(database=false, schema=false) -%}
    {% do return(tmp_relation) %}
{% endmacro %}


{% macro spark__alter_column_type(relation, column_name, new_column_type) -%}
  {% call statement('alter_column_type') %}
    alter table {{ relation }} alter column {{ column_name }} type {{ new_column_type }};
  {% endcall %}
{% endmacro %}


{% macro spark__alter_relation_add_remove_columns(relation, add_columns, remove_columns) %}

  {% if remove_columns %}
    {% if relation.is_delta %}
      {% set platform_name = 'Delta Lake' %}
    {% elif relation.is_iceberg %}
      {% set platform_name = 'Iceberg' %}
    {% else %}
      {% set platform_name = 'Apache Spark' %}
    {% endif %}
    {{ exceptions.raise_compiler_error(platform_name + ' does not support dropping columns from tables') }}
  {% endif %}

  {% if add_columns is none %}
    {% set add_columns = [] %}
  {% endif %}

  {% set sql -%}

     alter {{ relation.type }} {{ relation }}

       {% if add_columns %} add columns {% endif %}
            {% for column in add_columns %}
               {{ column.name }} {{ column.data_type }}{{ ',' if not loop.last }}
            {% endfor %}

  {%- endset -%}

  {% do run_query(sql) %}

{% endmacro %}

{% macro spark__check_schema_exists(information_schema, schema) -%}
  {% call statement('check_schema_exists', fetch_result=True, auto_begin=False) %}
    select count(*) from information_schema.schemata WHERE schema_name = '{{ schema }}'
  {% endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{% endmacro %}
