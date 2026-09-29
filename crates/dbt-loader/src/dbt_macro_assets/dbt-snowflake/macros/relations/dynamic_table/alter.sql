-- funcsign: (relation, snowflake_node_config, relation, string) -> string
{% macro snowflake__get_alter_dynamic_table_as_sql(
    existing_relation,
    configuration_changes,
    target_relation,
    sql
) -%}
    {{- log('Applying ALTER to: ' ~ existing_relation) -}}

    {% if configuration_changes.requires_full_refresh %}
        {{- get_replace_sql(existing_relation, target_relation, sql) -}}

    {% else %}

        {%- set scheduler = configuration_changes.scheduler -%}
        {{- snowflake__get_target_lag_warehouse_alter_sql('dynamic', existing_relation, configuration_changes, scheduler) -}}
        {%- set has_prior_set_changes = snowflake__target_lag_warehouse_alter_active(configuration_changes, scheduler) -%}

        {%- set immutable_where = configuration_changes.immutable_where -%}
        {%- if immutable_where and immutable_where.context -%}{{- log('Applying UPDATE IMMUTABLE WHERE to: ' ~ existing_relation) -}}{%- endif -%}
        {%- set cluster_by = configuration_changes.cluster_by -%}
        {%- if cluster_by and cluster_by.context -%}{{- log('Applying UPDATE CLUSTER BY to: ' ~ existing_relation) -}}{%- endif -%}

        {#- Handle setting or unsetting immutable_where -#}
        {% if immutable_where %}
        {% if has_prior_set_changes %};{% endif %}
        {% if immutable_where.context %}
        alter dynamic table {{ existing_relation }} set immutable where ({{ immutable_where.context }})
        {% else %}
        alter dynamic table {{ existing_relation }} unset immutable where
        {% endif %}
        {% endif %}

        {#- Track if we've had any previous ALTER statements for semicolon placement -#}
        {%- set has_prior_statements = has_prior_set_changes or immutable_where -%}

        {#- Handle CLUSTER BY changes (add/modify) -#}
        {% if cluster_by and cluster_by.context %}
        {% if has_prior_statements %};{% endif %}
        alter dynamic table {{ existing_relation }} cluster by ({{ cluster_by.context }})
        {% endif %}

        {#- Handle DROP CLUSTERING KEY when cluster_by is removed -#}
        {% if cluster_by and not cluster_by.context %}
        {%- if cluster_by -%}{{- log('Applying DROP CLUSTERING KEY to: ' ~ existing_relation) -}}{%- endif -%}
        {% if has_prior_statements %};{% endif %}
        alter dynamic table {{ existing_relation }} drop clustering key
        {% endif %}

    {%- endif -%}

{%- endmacro %}
