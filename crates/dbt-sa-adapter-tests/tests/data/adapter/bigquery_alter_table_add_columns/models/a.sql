{% set db_schema = target.schema %}
{% set db_name = target.database %}
{% do set_up(db_schema) %}

{% set relation_b = api.Relation.create(db_name, db_schema, "b") %}
{% set relation_c = api.Relation.create(db_name, db_schema, "c") %}
{{ print(db_schema) }}

{% set b_columns = adapter.get_columns_in_relation(relation_b) %}
{{ print("b_columns before") }}
{{ print(b_columns) }}

{% set missing_columns = adapter.get_missing_columns(relation_c, relation_b) %}
{{ print("missing_columns from b") }}
{{ print(missing_columns) }}
{% do adapter.alter_table_add_columns(relation_b, missing_columns) %}

{% set b_columns = adapter.get_columns_in_relation(relation_b) %}
{{ print("b_columns after") }}
{{ print(b_columns) }}

SELECT 1 as one
