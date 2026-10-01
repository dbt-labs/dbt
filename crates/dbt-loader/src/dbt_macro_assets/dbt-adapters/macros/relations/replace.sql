-- funcsign: (relation, relation, string) -> string
{% macro get_replace_sql(existing_relation, target_relation, sql) %}
    {{- log('Applying REPLACE to: ' ~ existing_relation) -}}
    {{- adapter.dispatch('get_replace_sql', 'dbt')(existing_relation, target_relation, sql) -}}
{% endmacro %}


-- funcsign: (relation, relation, string) -> string
{% macro default__get_replace_sql(existing_relation, target_relation, sql) %}

    {# /* use a create or replace statement if possible */ #}

    {% set is_replaceable = existing_relation.type == target_relation.type and existing_relation.can_be_replaced %}

    {% if is_replaceable and existing_relation.is_view %}
        {{ get_replace_view_sql(target_relation, sql) }}

    {% elif is_replaceable and existing_relation.is_table %}
        {{ get_replace_table_sql(target_relation, sql) }}

    {% elif is_replaceable and existing_relation.is_materialized_view %}
        {{ get_replace_materialized_view_sql(target_relation, sql) }}

    {# /* a create or replace statement is not possible, so try to stage and/or backup to be safe */ #}

    {# /* create target_relation as an intermediate relation, then swap it out with the existing one using a backup */ #}
    {%- elif target_relation.can_be_renamed and existing_relation.can_be_renamed -%}
        {{ get_create_intermediate_sql(target_relation, sql) }};
        {{ get_create_backup_sql(existing_relation) }};
        {{ get_rename_intermediate_sql(target_relation) }};
        {{ get_drop_backup_sql(existing_relation) }}

    {# /* create target_relation as an intermediate relation, then swap it out with the existing one without using a backup */ #}
    {%- elif target_relation.can_be_renamed -%}
        {{ get_create_intermediate_sql(target_relation, sql) }};
        {{ get_drop_sql(existing_relation) }};
        {{ get_rename_intermediate_sql(target_relation) }}

    {# /* create target_relation in place by first backing up the existing relation */ #}
    {%- elif existing_relation.can_be_renamed -%}
        {{ get_create_backup_sql(existing_relation) }};
        {{ get_create_sql(target_relation, sql) }};
        {{ get_drop_backup_sql(existing_relation) }}

    {# /* no renaming is allowed, so just drop and create */ #}
    {%- else -%}
        {{ get_drop_sql(existing_relation) }};
        {{ get_create_sql(target_relation, sql) }}

    {%- endif -%}

{% endmacro %}
