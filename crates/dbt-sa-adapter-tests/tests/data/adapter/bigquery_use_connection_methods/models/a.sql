{{ config(
    grant_access_to=[
      { 'project': database, 'dataset': schema},
    ],
) }}

{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}
{% set table_name = 'a' %}

{% do set_up(db_schema, table_name) %}

-- test get_dataset_location
{% set dataset_location = adapter.get_dataset_location(this) %}
{{ print('dataset_location') }}
{{ print(dataset_location) }}

-- test update_table_description
{{ print('update_table_description') }}
{% do adapter.update_table_description(model['database'], model['schema'], model['alias'], model['description']) %}

-- test get_columns_in_select_sql
{{ print('get_columns_in_select_sql') }}
--  this macro invokes adapter.get_columns_in_select_sql
{% set columns = get_columns_with_types_in_query_sql('select 1 as id') %}
{# {{ print(columns) }} #}
-- test get_column_schema_from_query
{{ print('get_column_schema_from_query') }}
-- this macro invokes adapter.get_column_schema_from_query
{% set columns = get_column_schema_from_query('select 1 as id') %}
{# {{ print(columns) }} #}

-- test grant_access_to (incomplete)
{{ print('grant_access_to') }}
{{ print(config.get('grant_access_to')) }}

-- test list_relations_without_caching
{{ print('list_relations_without_caching') }}
{% set relations = adapter.list_relations_without_caching(this) %}
{# {{ print(relations) }} #}

select 1 as id
