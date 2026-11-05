{{ config(materialized='view') }}
{% for value in [1, 2] %}
select {{ probe_value(value) }} as value from {{ ref('base') }}
{% if not loop.last %}union all{% endif %}
{% endfor %}
