{% materialization interactive_table, adapter='snowflake' %}

    {% set query_tag = set_query_tag() %}

    {% set existing_relation = load_cached_relation(this) %}
    {% set target_relation = this.incorporate(type=this.InteractiveTable) %}

    {{ run_hooks(pre_hooks) }}

    {% set build_sql = interactive_table_get_build_sql(existing_relation, target_relation) %}

    {% if build_sql == '' %}
        {{ interactive_table_execute_no_op(target_relation) }}
    {% else %}
        {{ interactive_table_execute_build_sql(build_sql, existing_relation, target_relation) }}
    {% endif %}

    {{ run_hooks(post_hooks) }}

    {% do unset_query_tag(query_tag) %}

    {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}


-- funcsign: (optional[relation], relation) -> string
{% macro interactive_table_get_build_sql(existing_relation, target_relation) %}

    {% set full_refresh_mode = should_full_refresh() %}

    {#- determine the scenario we're in: create, full refresh, alter -#}
    {% if existing_relation is none %}
        {% set build_sql = get_create_sql(target_relation, sql) %}
    {% elif full_refresh_mode or not existing_relation.is_interactive_table %}
        {% set build_sql = get_replace_sql(existing_relation, target_relation, sql) %}
    {% else %}

        {#- get config options -#}
        {% set on_configuration_change = config.get('on_configuration_change', 'apply') %} {# DIVERGENCE: core does not default to `apply` here because it sets it elsewhere in the Python code #}
        {% set configuration_changes = snowflake__get_interactive_table_configuration_changes(existing_relation, config) %}

        {% if configuration_changes is none %}
            {% set build_sql = '' %}
            {{ exceptions.warn("No configuration changes were identified on: `" ~ target_relation ~ "`. Continuing.") }}

        {% elif on_configuration_change == 'apply' %}
            {% set build_sql = snowflake__get_alter_interactive_table_as_sql(existing_relation, configuration_changes, target_relation, sql) %}
        {% elif on_configuration_change == 'continue' %}
            {% set build_sql = '' %}
            {{ exceptions.warn("Configuration changes were identified and `on_configuration_change` was set to `continue` for `" ~ target_relation ~ "`") }}
        {% elif on_configuration_change == 'fail' %}
            {{ exceptions.raise_fail_fast_error("Configuration changes were identified and `on_configuration_change` was set to `fail` for `" ~ target_relation ~ "`") }}

        {% else %}
            {#- this only happens if the user provides a value other than `apply`, 'continue', 'fail' -#}
            {{ exceptions.raise_compiler_error("Unexpected configuration scenario: `" ~ on_configuration_change ~ "`") }}

        {% endif %}

    {% endif %}

    {% do return(build_sql) %}

{% endmacro %}


-- funcsign: (relation) -> string
{% macro interactive_table_execute_no_op(relation) %}
    {% do store_raw_result(
        name="main",
        message="skip " ~ relation,
        code="skip",
        rows_affected="-1"
    ) %}
{% endmacro %}


-- funcsign: (string, optional[relation], relation) -> string
{% macro interactive_table_execute_build_sql(build_sql, existing_relation, target_relation) %}

    {% set grant_config = config.get('grants') %}

    {% call statement(name="main") %}
        {{ build_sql }}
    {% endcall %}

    {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
    {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

    {% do persist_docs(target_relation, model) %}

{% endmacro %}


-- funcsign: (relation, config) -> optional[snowflake_node_config]
{% macro snowflake__get_interactive_table_configuration_changes(existing_relation, new_config) -%}
    {# DIVERGENCE: core wraps adapter.describe_interactive_table in a snowflake__describe_interactive_table macro; Fusion dispatches every relation type through adapter.describe_relation in adapter_impl.rs, so there is no wrapper macro. #}
    {% set _existing_interactive_table = adapter.describe_relation(existing_relation) %}
    {% set _configuration_changes = existing_relation.interactive_table_config_changeset(_existing_interactive_table, new_config.model) %}
    {% do return(_configuration_changes) %}
{%- endmacro %}
