{% set relation = ref('a') %}
{{ print(relation) }}

{% set _ = adapter.create_schema(relation) %}

select * from {{ ref('a') }}
