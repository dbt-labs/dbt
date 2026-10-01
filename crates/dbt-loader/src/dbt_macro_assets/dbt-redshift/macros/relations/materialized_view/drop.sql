{# adapter.drop_without_cascade is only available in v1. v2 uses the more generic adapter.has_feature instead #}
{%- macro redshift__drop_materialized_view(relation) -%}
    drop materialized view if exists {{ relation }}
    {%- if dbt_version.startswith('2.') -%}
        {% if not adapter.has_feature('drop_without_cascade') %} cascade{% endif %}
    {%- else -%}
        {% if not redshift__drop_without_cascade() %} cascade{% endif %}
    {%- endif %}
{%- endmacro %}
