{% set current_schema = '' %}
{% if execute %}
    {% set current_schema = run_query("select current_schema();")[0][0] %}
{% endif %}

select
    '{{ target.schema }}' as target_schema,
    '{{ current_schema }}' as current_schema