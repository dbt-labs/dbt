{% macro redshift__use_show_apis() %}
    {% if dbt_version.startswith('2.') %}
        {{ return(adapter.has_feature("datasharing")) }}
    {% else %}
        {{ return(adapter.use_show_apis()) }}
    {% endif %}
{% endmacro %}

{% macro redshift__use_grants_extended() %}
    {{ return(adapter.behavior.redshift_grants_extended.no_warn) }}
{% endmacro %}

{% macro redshift__drop_without_cascade() %}
    {{ return(adapter.drop_without_cascade()) }}
{% endmacro %}
