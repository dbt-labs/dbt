{% set db_name = "db_name" %}
{% set db_schema = "schema" %}


-- `wrong_identifier` is ignored, then `identifier` component defaults to false
{% set relation_with_partial_explicit_quoting = api.Relation.create(db_name, db_schema, 'a', quote_policy={
    "database": true,
    "schema": true,
    "wrong_identifier": true
}) %}
{{ print("relation_with_partial_explicit_quoting: "  ~ relation_with_partial_explicit_quoting) }}

{% set relation_with_full_explicit_quoting = api.Relation.create(db_name, db_schema, 'a', quote_policy={
    "database": true,
    "schema": true,
    "identifier": true
}) %}
{{ print("relation_with_full_explicit_quoting: "  ~ relation_with_full_explicit_quoting) }}

{% set relation_with_config_quoting = api.Relation.create(db_name, db_schema, 'a') %}
{{ print("relation_with_config_quoting: "  ~ relation_with_config_quoting) }}

select 1 as one
