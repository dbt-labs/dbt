{# UTF-8: café #}
{% macro doubled(n) %}{{ n * 2 }}{% endmacro %}
{% for n in [1, 2] %}
select {{ doubled(n) }} as n{% if not loop.last %} union all{% endif %}
{% endfor %}
{% for unused in [] %}-- skipped{% endfor %}
