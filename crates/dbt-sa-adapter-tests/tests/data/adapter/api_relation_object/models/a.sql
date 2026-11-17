{% set relation = api.Relation.create(target.database, target.schema, "a") %}
{{ print(relation) }}

SELECT 1 as one
