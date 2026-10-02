{% macro singlestore__create_table_as(temporary, relation, compiled_code, language='sql') -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {%- set primary_key = config.get('primary_key', none) -%}
  {%- set sort_key = config.get('sort_key', none) -%}
  {%- set shard_key = config.get('shard_key', none) -%}
  {%- set unique_table_key = config.get('unique_table_key', none) -%}
  {%- set fulltext_key = config.get('fulltext_key', none) -%}
  {%- set reference = config.get('reference', False) -%}
  {%- set storage_type = config.get('storage_type', '') -%}
  {%- set charset = config.get('charset', none) -%}
  {%- set collation = config.get('collation', none) -%}

  {# Normalize keys to lists #}
  {%- if primary_key is string -%}{%- set primary_key = [primary_key] -%}{%- endif -%}
  {%- if sort_key is string -%}{%- set sort_key = [sort_key] -%}{%- endif -%}
  {%- if shard_key is string -%}{%- set shard_key = [shard_key] -%}{%- endif -%}
  {%- if unique_table_key is string -%}{%- set unique_table_key = [unique_table_key] -%}{%- endif -%}
  {%- if fulltext_key is string -%}{%- set fulltext_key = [fulltext_key] -%}{%- endif -%}

  {%- set create_definition_list = [] -%}

  {%- if not temporary and primary_key | length -%}
    {%- set quoted_pk = [] -%}
    {%- for col in primary_key -%}
      {%- do quoted_pk.append(adapter.quote(col)) -%}
    {%- endfor -%}
    {%- do create_definition_list.append('PRIMARY KEY (' ~ quoted_pk | join(', ') ~ ')') -%}
  {%- endif -%}

  {%- if sort_key | length -%}
    {%- set quoted_sk = [] -%}
    {%- for col in sort_key -%}
      {%- do quoted_sk.append(adapter.quote(col)) -%}
    {%- endfor -%}
    {%- if temporary or storage_type | lower == 'rowstore' -%}
      {%- do create_definition_list.append('KEY (' ~ quoted_sk | join(', ') ~ ')') -%}
    {%- else -%}
      {%- do create_definition_list.append('SORT KEY (' ~ quoted_sk | join(', ') ~ ')') -%}
    {%- endif -%}
  {%- endif -%}

  {%- if not temporary -%}
    {%- if shard_key | length -%}
      {%- set quoted_shk = [] -%}
      {%- for col in shard_key -%}
        {%- do quoted_shk.append(adapter.quote(col)) -%}
      {%- endfor -%}
      {%- do create_definition_list.append('SHARD KEY (' ~ quoted_shk | join(', ') ~ ')') -%}
    {%- elif unique_table_key | length and not reference -%}
      {# SingleStore restriction: unique keys must contain all shard key columns. Default shard key to unique key. #}
      {%- set quoted_shk = [] -%}
      {%- for col in unique_table_key -%}
        {%- do quoted_shk.append(adapter.quote(col)) -%}
      {%- endfor -%}
      {%- do create_definition_list.append('SHARD KEY (' ~ quoted_shk | join(', ') ~ ')') -%}
    {%- endif -%}

    {%- if unique_table_key | length and unique_table_key != primary_key -%}
      {%- set quoted_uk = [] -%}
      {%- for col in unique_table_key -%}
        {%- do quoted_uk.append(adapter.quote(col)) -%}
      {%- endfor -%}
      {%- do create_definition_list.append('UNIQUE KEY (' ~ quoted_uk | join(', ') ~ ')') -%}
    {%- endif -%}

    {%- if fulltext_key | length -%}
      {%- set quoted_ftk = [] -%}
      {%- for col in fulltext_key -%}
        {%- do quoted_ftk.append(adapter.quote(col)) -%}
      {%- endfor -%}
      {%- do create_definition_list.append('FULLTEXT (' ~ quoted_ftk | join(', ') ~ ')') -%}
    {%- endif -%}
  {%- endif -%}

  {%- if create_definition_list | length -%}
    {%- set create_definition_str = '(' ~ create_definition_list | join(', ') ~ ')' -%}
  {%- elif reference or temporary -%}
    {%- set create_definition_str = '' -%}
  {%- else -%}
    {%- set create_definition_str = '(SHARD KEY ())' -%}
  {%- endif -%}

  {%- set table_type = '' -%}
  {%- if temporary -%}
    {%- if sort_key | length -%}
      {%- set table_type = 'temporary' -%}
    {%- else -%}
      {%- set table_type = 'rowstore temporary' -%}
    {%- endif -%}
  {%- elif storage_type | lower == 'rowstore' -%}
    {%- set table_type = 'rowstore' -%}
  {%- endif -%}

  {%- if reference -%}
    {%- set table_type = (table_type ~ ' reference') | trim -%}
  {%- endif -%}

  {%- set charset_str = '' -%}
  {%- if charset is not none -%}
    {%- set charset_str = charset_str ~ ' CHARACTER SET ' ~ charset -%}
  {%- endif -%}
  {%- if collation is not none -%}
    {%- set charset_str = charset_str ~ ' COLLATE ' ~ collation -%}
  {%- endif -%}

  {{ sql_header if sql_header is not none }}

  create {{ table_type }} table {{ relation.render() }}
    {% if create_definition_str | length %}{{ create_definition_str }}{% endif %}
    {{ charset_str }}
  as
    {{ singlestore__strip_db_limit_aliases(compiled_code) }}
{%- endmacro %}

{% macro singlestore__create_view_as(relation, sql) -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {{ sql_header if sql_header is not none }}
  create view {{ relation.render() }} as
    {{ singlestore__strip_db_limit_aliases(sql) }}
{%- endmacro %}

{% macro singlestore__strip_db_limit_aliases(sql) -%}
  {%- if adapter.clean_up_limit_alias is defined -%}
    {{ return(adapter.clean_up_limit_alias(sql)) }}
  {%- else -%}
    {{ return(sql) }}
  {%- endif -%}
{%- endmacro %}

{% macro singlestore__drop_relation(relation) -%}
  {% call statement('drop_relation') -%}
    {% if relation.is_view or relation.type == 'view' -%}
      drop view if exists {{ relation.render() }}
    {%- else -%}
      drop table if exists {{ relation.render() }}
    {%- endif %}
  {%- endcall %}
{%- endmacro %}

{% macro singlestore__truncate_relation(relation) -%}
  {% call statement('truncate_relation') -%}
    truncate table {{ relation.render() }}
  {%- endcall %}
{%- endmacro %}

{% macro singlestore__rename_relation(from_relation, to_relation) -%}
  {% call statement('drop_to_relation_if_exists') -%}
    {% if to_relation.is_view or to_relation.type == 'view' -%}
      drop view if exists {{ to_relation.render() }}
    {%- else -%}
      drop table if exists {{ to_relation.render() }}
    {%- endif %}
  {%- endcall %}
  {% call statement('rename_relation') -%}
    alter table {{ from_relation.render() }} rename to {{ to_relation.render() }}
  {%- endcall %}
{%- endmacro %}

{% macro singlestore__check_schema_exists(information_schema, schema) -%}
  {% call statement('check_schema_exists', fetch_result=True) -%}
    select schema_name from information_schema.schemata where lower(schema_name) = lower('{{ schema }}')
  {%- endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{%- endmacro %}

{% macro singlestore__list_schemas(database) -%}
  {% call statement('list_schemas', fetch_result=True) -%}
    show databases
  {%- endcall %}
  {{ return(load_result('list_schemas').table) }}
{%- endmacro %}

{% macro singlestore__list_relations_without_caching(schema_relation) -%}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    select
      table_schema as `database`,
      table_name as `name`,
      table_schema as `schema`,
      case when table_type = 'VIEW' then 'view'
           else 'table'
      end as `type`
    from information_schema.tables
    where table_schema = '{{ schema_relation.schema }}'
  {%- endcall %}
  {{ return(load_result('list_relations_without_caching').table) }}
{%- endmacro %}

{% macro singlestore__get_columns_in_relation(relation) -%}
  {% call statement('get_columns_in_relation', fetch_result=True) %}
    select
      column_name,
      data_type,
      character_maximum_length,
      numeric_precision,
      numeric_scale
    from information_schema.columns
    where table_name = '{{ relation.identifier }}'
      {% if relation.schema %}
      and table_schema = '{{ relation.schema }}'
      {% endif %}
    order by ordinal_position
  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {{ return(sql_convert_columns_in_relation(table)) }}
{%- endmacro %}
