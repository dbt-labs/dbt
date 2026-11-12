{% materialization dynamic_table, adapter='snowflake' %}

    {% set query_tag = set_query_tag() %}

    {% set existing_relation = load_cached_relation(this) %}
    {% set target_relation = this.incorporate(type=this.DynamicTable) %}

    {% if snowflake__dynamic_table_is_custom_incremental() %}
        {% do snowflake__validate_custom_incremental_config() %}
    {% endif %}

    {{ run_hooks(pre_hooks) }}

    {% set build_sql = dynamic_table_get_build_sql(existing_relation, target_relation) %}

    {% if build_sql == '' %}
        {{ dynamic_table_execute_no_op(target_relation) }}
    {% else %}
        {{ dynamic_table_execute_build_sql(build_sql, existing_relation, target_relation) }}
    {% endif %}

    {%- set dynamic_table = target_relation.from_config(config.model) -%}
    {%- if dynamic_table.scheduler | string | upper == 'DISABLE' -%}
        {% call statement(name="refresh") %}
            {{ snowflake__refresh_dynamic_table(target_relation) }}
        {% endcall %}
    {%- endif -%}

    {{ run_hooks(post_hooks) }}

    {% do unset_query_tag(query_tag) %}

    {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}


{% macro dynamic_table_get_build_sql(existing_relation, target_relation) %}

    {% set full_refresh_mode = should_full_refresh() %}

    -- determine the scenario we're in: create, full_refresh, create-or-alter, alter, refresh data
    {% if existing_relation is none %}
        {% set build_sql = get_create_sql(target_relation, sql) %}
    {% elif full_refresh_mode or not existing_relation.is_dynamic_table %}
        {% set build_sql = get_replace_sql(existing_relation, target_relation, sql) %}
    {% else %}

        {#- Opt-in query-evolution path: SQL edits deploy in place via CREATE OR ALTER, without
            --full-refresh; otherwise fall through to the config-change handling below. DIVERGENCE:
            v1 gates on adapter.dynamic_table_create_or_alter_enabled; Fusion uses a macro. -#}
        {% if snowflake__dynamic_table_create_or_alter_enabled(config.model) %}
            {% set build_sql = snowflake__get_create_or_alter_dynamic_table_sql(existing_relation, target_relation, sql) %}

        {% else %}

            -- get config options
            {% set on_configuration_change = config.get('on_configuration_change') %}
            {% set existing_dynamic_table = snowflake__describe_dynamic_table_for_changes(existing_relation, config) %}
            {% set configuration_changes = snowflake__get_dynamic_table_configuration_changes(existing_relation, config, existing_dynamic_table) %}

            {#- A switch between a custom incremental and a regular dynamic table is a change the
                changeset can miss (a desired AUTO refresh_mode matches anything). -#}
            {% set kind_changed = snowflake__dynamic_table_kind_changed(existing_dynamic_table) %}

            {% if configuration_changes is none and not kind_changed %}
                {% set build_sql = '' %}
                {% if snowflake__dynamic_table_is_custom_incremental() %}
                    {{ exceptions.warn("No configuration changes were identified on: `" ~ target_relation ~ "`. Continuing. Changes to the model SQL of a custom incremental dynamic table are only deployed with --full-refresh.") }}
                {% else %}
                    {{ exceptions.warn("No configuration changes were identified on: `" ~ target_relation ~ "`. Continuing.") }}
                {% endif %}

            {% elif on_configuration_change == 'apply' %}
                {% if kind_changed %}
                    {{ log("Replacing " ~ target_relation ~ " because it switches between a custom incremental and a regular dynamic table.") }}
                    {% set build_sql = get_replace_sql(existing_relation, target_relation, sql) %}
                {% else %}
                    {% set build_sql = snowflake__get_alter_dynamic_table_as_sql(existing_relation, configuration_changes, target_relation, sql) %}
                {% endif %}
            {% elif on_configuration_change == 'continue' %}
                {% set build_sql = '' %}
                {{ exceptions.warn("Configuration changes were identified and `on_configuration_change` was set to `continue` for `" ~ target_relation ~ "`") }}
            {% elif on_configuration_change == 'fail' %}
                {{ exceptions.raise_fail_fast_error("Configuration changes were identified and `on_configuration_change` was set to `fail` for `" ~ target_relation ~ "`") }}

            {% else %}
                -- this only happens if the user provides a value other than `apply`, 'continue', 'fail'
                {{ exceptions.raise_compiler_error("Unexpected configuration scenario: `" ~ on_configuration_change ~ "`") }}

            {% endif %}

        {% endif %}

    {% endif %}

    {% do return(build_sql) %}

{% endmacro %}


{% macro dynamic_table_execute_no_op(relation) %}
    {% do store_raw_result(
        name="main",
        message="skip " ~ relation,
        code="skip",
        rows_affected="-1"
    ) %}
{% endmacro %}


{% macro dynamic_table_execute_build_sql(build_sql, existing_relation, target_relation) %}

    {% set grant_config = config.get('grants') %}

    {% call statement(name="main") %}
        {{ build_sql }}
    {% endcall %}

    {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
    {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

    {% do persist_docs(target_relation, model) %}

{% endmacro %}


{% macro snowflake__describe_dynamic_table_for_changes(existing_relation, new_config) -%}
    {%- set _include_transient = new_config.get("transient") is not none -%}
    {# DIVERGENCE: core wraps adapter.describe_dynamic_table in a snowflake__describe_dynamic_table macro; Fusion dispatches every relation type through adapter.describe_relation in adapter_impl.rs, so there is no wrapper macro. #}
    {% do return(adapter.describe_relation(existing_relation, include_transient=_include_transient)) %}
{%- endmacro %}


{% macro snowflake__get_dynamic_table_configuration_changes(existing_relation, new_config, existing_dynamic_table=none) -%}
    {#- DIVERGENCE: `existing_dynamic_table` lets the materialization reuse its describe result instead of describing twice. -#}
    {%- set _existing_dynamic_table = existing_dynamic_table if existing_dynamic_table is not none else snowflake__describe_dynamic_table_for_changes(existing_relation, new_config) -%}
    {% set _configuration_changes = existing_relation.dynamic_table_config_changeset(_existing_dynamic_table, new_config.model) %}
    {% do return(_configuration_changes) %}
{%- endmacro %}
