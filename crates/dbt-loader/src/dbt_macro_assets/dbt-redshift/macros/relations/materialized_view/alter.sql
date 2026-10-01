{% macro redshift__get_alter_materialized_view_as_sql(
    relation,
    configuration_changes,
    sql,
    existing_relation,
    backup_relation,
    intermediate_relation
) %}

    -- apply a full refresh immediately if needed
    {% if configuration_changes.requires_full_refresh %}

        {{ get_replace_sql(existing_relation, relation,  sql) }}

    -- otherwise apply individual changes as needed
    {% else %}

        {%- set autorefresh = configuration_changes.autorefresh -%}
        {%- if autorefresh -%}{{- log('Applying UPDATE AUTOREFRESH to: ' ~ relation) -}}{%- endif -%}

        alter materialized view {{ relation }}
            {# DIVERGENCE: autorefresh in v2 is the same as autorefresh.context in v1 #}
            auto refresh {% if autorefresh %}yes{% else %}no{% endif %}

    {%- endif -%}

{% endmacro %}
