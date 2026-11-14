{#- MariaDB has no catalog level: a dbt `schema` is a MariaDB database, and
    relations render as `schema`.`identifier`. -#}

{% macro mariadb__create_schema(relation) -%}
  {%- call statement('create_schema') -%}
    create schema if not exists {{ relation.without_identifier().include(database=False) }}
  {%- endcall -%}
{% endmacro %}

{% macro mariadb__drop_schema(relation) -%}
  {%- call statement('drop_schema') -%}
    drop schema if exists {{ relation.without_identifier().include(database=False) }}
  {%- endcall -%}
{% endmacro %}

{% macro mariadb__create_table_as(temporary, relation, sql) -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {{ sql_header if sql_header is not none }}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(sql) }}
  {%- endif %}
  create or replace {% if temporary -%} temporary {%- endif %} table {{ relation }}
    {%- if contract_config.enforced and not temporary and mariadb__is_model_target(relation) %}
    {{ get_table_columns_and_constraints() }}
    {%- elif contract_config.enforced and not temporary %}
    {#- Intermediate table: its foreign keys get generated names, see foreign_keys.sql. #}
    {{ mariadb__table_columns_and_constraints(unnamed_foreign_keys=True) }}
    {%- elif contract_config.enforced %}
    {#- TEMPORARY tables cannot hold foreign keys: declare the contract
        types only, so on_schema_change compares the same types. #}
    {{ mariadb__get_table_columns() }}
    {%- endif %}
  as
    {%- if contract_config.enforced %}
    {{ get_select_subquery(sql) }}
    {%- else %}
    {{ sql }}
    {%- endif %}
{%- endmacro %}

{% macro mariadb__get_table_columns() -%}
    (
    {%- for column in model['columns'].values() %}
      {{ adapter.quote(column['name']) if column.get('quote') else column['name'] }} {{ column['data_type'] }}{{ "," if not loop.last }}
    {%- endfor %}
    )
{%- endmacro %}

{% macro mariadb__create_view_as(relation, sql) -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {{ sql_header if sql_header is not none }}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{- get_assert_columns_equivalent(sql) }}
  {%- endif %}
  create or replace view {{ relation }} as
    {{ sql }}
{%- endmacro %}

{#- MariaDB ignores DROP ... CASCADE and refuses to drop a table that a
    foreign key references. Drop those foreign keys first, as Postgres'
    CASCADE does, so a referenced model can still be rebuilt. -#}
{% macro mariadb__drop_referencing_foreign_keys(relation) -%}
  {% call statement('referencing_foreign_keys', fetch_result=True, auto_begin=False) -%}
    select constraint_schema, table_name, constraint_name
    from information_schema.referential_constraints
    where unique_constraint_schema = '{{ relation.schema | replace("\\", "\\\\") | replace("'", "''") }}'
      and referenced_table_name = '{{ relation.identifier | replace("\\", "\\\\") | replace("'", "''") }}'
  {%- endcall %}
  {% for row in load_result('referencing_foreign_keys').table.rows %}
    {% call statement('drop_referencing_foreign_key', auto_begin=False) -%}
      alter table {{ adapter.quote(row[0]) }}.{{ adapter.quote(row[1]) }} drop foreign key {{ adapter.quote(row[2]) }}
    {%- endcall %}
  {% endfor %}
{% endmacro %}

{% macro mariadb__drop_relation(relation) -%}
  {% if relation.type == 'table' %}
    {{ mariadb__drop_referencing_foreign_keys(relation) }}
  {% endif %}
  {% call statement('drop_relation', auto_begin=False) -%}
    drop {{ relation.type }} if exists {{ relation }}
  {%- endcall %}
{% endmacro %}

{% macro mariadb__rename_relation(from_relation, to_relation) -%}
  {%- set named_foreign_keys = mariadb__named_foreign_keys() -%}
  {%- if named_foreign_keys and mariadb__is_model_target(from_relation) -%}
    {{ mariadb__drop_named_foreign_keys(from_relation, named_foreign_keys) }}
  {%- endif -%}
  {#- RENAME TABLE also renames views. -#}
  {% call statement('rename_relation') -%}
    rename table {{ from_relation }} to {{ to_relation }}
  {%- endcall %}
  {%- if named_foreign_keys and mariadb__is_model_target(to_relation) -%}
    {{ mariadb__restore_foreign_key_names(to_relation, named_foreign_keys) }}
  {%- endif -%}
{% endmacro %}

{% macro mariadb__truncate_relation(relation) -%}
  {% call statement('truncate_relation') -%}
    truncate table {{ relation }}
  {%- endcall %}
{% endmacro %}

{% macro mariadb__list_schemas(database) -%}
  {% call statement('list_schemas', fetch_result=True, auto_begin=False) -%}
    select schema_name as name from information_schema.schemata
  {%- endcall %}
  {{ return(load_result('list_schemas').table) }}
{% endmacro %}

{% macro mariadb__check_schema_exists(information_schema, schema) -%}
  {% call statement('check_schema_exists', fetch_result=True, auto_begin=False) -%}
    select count(*) from information_schema.schemata
    where schema_name = '{{ schema | replace("\\", "\\\\") | replace("'", "''") }}'
  {%- endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{% endmacro %}

{% macro mariadb__information_schema_name(database) -%}
  information_schema
{%- endmacro %}

{% macro mariadb__current_timestamp() -%}
  current_timestamp()
{%- endmacro %}

{% macro mariadb__get_columns_in_relation(relation) -%}
  {#- SHOW COLUMNS, unlike information_schema.columns, also sees the session's
      TEMPORARY tables (incremental and snapshot staging relations). -#}
  {% call statement('get_columns_in_relation', fetch_result=True) %}
    show columns from {{ relation }}
  {% endcall %}
  {% set columns = [] %}
  {% for row in load_result('get_columns_in_relation').table %}
    {#- Keep the server's type text verbatim (e.g. `int(10) unsigned`,
        `datetime(6)`, `enum('a','b')`): it is valid MariaDB DDL as-is. -#}
    {% do columns.append(api.Column.create(row['Field'], row['Type'])) %}
  {% endfor %}
  {{ return(columns) }}
{%- endmacro %}

{% macro mariadb__list_relations_without_caching(schema_relation) -%}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    select
      '' as `database`,
      table_name as name,
      table_schema as `schema`,
      case when table_type = 'VIEW' then 'view' else 'table' end as type
    from information_schema.tables
    where table_schema = '{{ schema_relation.schema | replace("\\", "\\\\") | replace("'", "''") }}'
      and table_type in ('BASE TABLE', 'SYSTEM VERSIONED', 'VIEW')
  {%- endcall %}
  {{ return(load_result('list_relations_without_caching').table) }}
{%- endmacro %}
