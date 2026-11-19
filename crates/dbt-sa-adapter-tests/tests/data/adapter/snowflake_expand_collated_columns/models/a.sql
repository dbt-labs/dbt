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
{% for col in columns %}
  {{ print("  " ~ col.name ~ ": " ~ col.data_type ~ " collation=" ~ col.collation) }}
{% endfor %}

{% set target_columns = adapter.get_columns_in_relation(target_relation) %}
{{ print("target columns") }}
{% for col in target_columns %}
  {{ print("  " ~ col.name ~ ": " ~ col.data_type ~ " collation=" ~ col.collation) }}
{% endfor %}

{#-- expand_target_column_types should preserve collation on the ALTER statement #}
{% do adapter.expand_target_column_types(
        from_relation=tmp_relation,
        to_relation=target_relation) %}

{% set target_columns_after = adapter.get_columns_in_relation(target_relation) %}
{{ print("after target columns") }}
{% for col in target_columns_after %}
  {{ print("  " ~ col.name ~ ": " ~ col.data_type ~ " collation=" ~ col.collation) }}
{% endfor %}

select cast('1' as varchar(1)) as id
