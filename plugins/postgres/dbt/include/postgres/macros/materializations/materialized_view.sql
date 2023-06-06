{% macro postgres__get_alter_materialized_view_as_sql(
    relation,
    configuration_changes,
    sql,
    existing_relation,
    backup_relation,
    intermediate_relation
) %}

    -- apply a full refresh immediately if needed
    {% if configuration_changes.requires_full_refresh() %}

        {{ get_replace_materialized_view_as_sql(relation, sql, existing_relation, backup_relation, intermediate_relation) }}

    -- otherwise apply individual changes as needed
    {% else %}

        {{ postgres__update_indexes_on_materialized_view(relation, configuration_changes.indexes) }}

    {%- endif -%}

{% endmacro %}


{% macro postgres__get_create_materialized_view_as_sql(relation, sql) %}
    create materialized view if not exists {{ relation }} as {{ sql }};

    {% for _index_dict in config.get('indexes', []) -%}
        {{- get_create_index_sql(relation, _index_dict) -}}
    {%- endfor -%}

{% endmacro %}


{% macro postgres__get_replace_materialized_view_as_sql(relation, sql, existing_relation, backup_relation, intermediate_relation) %}
    {{- get_create_materialized_view_as_sql(intermediate_relation, sql) -}}

    {% if existing_relation is not none %}
        alter materialized view {{ existing_relation }} rename to {{ backup_relation.include(database=False, schema=False) }};
    {% endif %}

    alter materialized view {{ intermediate_relation }} rename to {{ relation.include(database=False, schema=False) }};

{% endmacro %}


{% macro postgres__get_materialized_view_configuration_changes(existing_relation, new_config) %}
    {% set _existing_materialized_view = postgres__describe_materialized_view(existing_relation) %}
    {% set _configuration_changes = existing_relation.get_relation_config_change_collection(_existing_materialized_view, new_config) %}
    {% do return(_configuration_changes) %}
{% endmacro %}


{% macro postgres__refresh_materialized_view(relation) %}
    refresh materialized view {{ relation }};
{% endmacro %}


{%- macro postgres__update_indexes_on_materialized_view(relation, index_changes) -%}
    {{- log("Applying UPDATE INDEXES to: " ~ relation) -}}

    {%- for _index_change in index_changes -%}
        {%- set _index = _index_change.context -%}

        {%- if _index_change.action == "drop" -%}

            {{ postgres__get_drop_index_sql(relation, _index.name) }};

        {%- elif _index_change.action == "create" -%}

            {{ postgres__get_create_index_sql(relation, _index.as_user_config()) }}

        {%- else -%}

            {{- exceptions.raise_compiler_error(
                "Unsupported action supplied to postgres__update_indexes_on_materialized_view: " ~ _index_change.action)
            -}}

        {%- endif -%}

    {%- endfor -%}

{%- endmacro -%}


{% macro postgres__describe_materialized_view(relation) %}
    -- for now just get the indexes, we don't need the name or the query yet
    {% set _indexes = run_query(get_show_indexes_sql(relation)) %}
    {% do return({'indexes': _indexes}) %}
{% endmacro %}
