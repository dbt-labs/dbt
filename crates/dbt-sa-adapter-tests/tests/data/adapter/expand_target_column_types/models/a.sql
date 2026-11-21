{{config(materialized='table') }}

{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}
{% do set_up(db_schema) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set tmp_relation = api.Relation.create(db_name, db_schema, "a") %}
{% set target_relation = api.Relation.create(db_name, db_schema, "b") %}

{% set columns = adapter.get_columns_in_relation(tmp_relation) %}
{{ print("source columns") }}
{# {{ print(columns) }} #}

{% set target_columns = adapter.get_columns_in_relation(target_relation) %}
{{ print("target columns") }}
{# {{ print(target_columns) }} #}

{#-- Use expand_target_column_types to change the data type of the column(s) that have changed #}
{% do adapter.expand_target_column_types(
        from_relation=tmp_relation,
        to_relation=target_relation) %}


{% set columns = adapter.get_columns_in_relation(tmp_relation) %}
{{ print("after source columns") }}
{# {{ print(columns) }} #}

{% set target_columns = adapter.get_columns_in_relation(target_relation) %}
{{ print("after target columns") }}
{# {{ print(target_columns) }} #}


select cast('1' as varchar(1)) as id
