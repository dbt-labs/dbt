{% set table_name_1 = 'c_1' %}
{% set db_name = target.database %}
{% set db_schema = t_schema_name() %}

{% do set_up(db_schema, [table_name_1]) %}

-- This should return two relations
{% set relations = adapter.get_relations_by_pattern(db_schema ~ '%', '%c%') %}
{% for relation in relations %}
    {{ print(relation) }}
{% endfor %}

-- no results, filtered out by excluded_schemas
{{ print("excluded_schemas: " ~ db_schema | upper) }}
{% set relations = adapter.get_relations_by_pattern(db_schema ~ '%', '%c%', exclude='', database=db_name, quote_table=False, excluded_schemas=[db_schema | upper]) %}
{% for relation in relations %}
    {{ print(relation) }}
{% endfor %}

select 1 as one
