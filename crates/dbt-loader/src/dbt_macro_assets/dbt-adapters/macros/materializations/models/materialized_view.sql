{% materialization materialized_view, default %}
    {% set existing_relation = load_cached_relation(this) %}
    {% set target_relation = this.incorporate(type=this.MaterializedView) %}
    {% set intermediate_relation = make_intermediate_relation(target_relation) %}
    {% set backup_relation_type = target_relation.MaterializedView if existing_relation is none else existing_relation.type %}
    {% set backup_relation = make_backup_relation(target_relation, backup_relation_type) %}

    {{ materialized_view_setup(backup_relation, intermediate_relation, pre_hooks) }}

        {% set build_sql = materialized_view_get_build_sql(existing_relation, target_relation, backup_relation, intermediate_relation) %}

        {% if build_sql == '' %}
            {{ materialized_view_execute_no_op(target_relation) }}
        {% else %}
            {{ materialized_view_execute_build_sql(build_sql, existing_relation, target_relation, post_hooks) }}
        {% endif %}

    {{ materialized_view_teardown(backup_relation, intermediate_relation, post_hooks) }}

    {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}


-- funcsign: (relation, relation, list[hook]) -> string
{% macro materialized_view_setup(backup_relation, intermediate_relation, pre_hooks) %}

    -- backup_relation and intermediate_relation should not already exist in the database
    -- it's possible these exist because of a previous run that exited unexpectedly
    {% set preexisting_backup_relation = load_cached_relation(backup_relation) %}
    {% set preexisting_intermediate_relation = load_cached_relation(intermediate_relation) %}

    -- drop the temp relations if they exist already in the database
    {{ drop_relation_if_exists(preexisting_backup_relation) }}
    {{ drop_relation_if_exists(preexisting_intermediate_relation) }}

    {{ run_hooks(pre_hooks, inside_transaction=False) }}

{% endmacro %}


-- funcsign: (optional[relation], optional[relation], list[hook]) -> string
{% macro materialized_view_teardown(backup_relation, intermediate_relation, post_hooks) %}

    -- drop the temp relations if they exist to leave the database clean for the next run
    {{ drop_relation_if_exists(backup_relation) }}
    {{ drop_relation_if_exists(intermediate_relation) }}

    {{ run_hooks(post_hooks, inside_transaction=False) }}

{% endmacro %}


-- funcsign: (optional[relation], relation, relation, relation) -> string
{% macro materialized_view_get_build_sql(existing_relation, target_relation, backup_relation, intermediate_relation) %}

    {% set full_refresh_mode = should_full_refresh() %}

    -- determine the scenario we're in: create, full_refresh, alter, refresh data
    {% if existing_relation is none %}
        {% set build_sql = get_create_materialized_view_as_sql(target_relation, sql) %}
    {% elif full_refresh_mode or not existing_relation.is_materialized_view %}
        {% set build_sql = get_replace_sql(existing_relation, target_relation, sql) %}
    {% else %}

        -- get config options
        {% set on_configuration_change = config.get('on_configuration_change', 'apply') %} {# DIVERGENCE: core does not default to `apply` here because it sets it elsewhere in the Python code #}
        {% set configuration_changes = get_materialized_view_configuration_changes(existing_relation, config) %}

        {% if configuration_changes is none %}
            {% set build_sql = refresh_materialized_view(target_relation) %}

        {% elif on_configuration_change == 'apply' %}
            {% set build_sql = get_alter_materialized_view_as_sql(target_relation, configuration_changes, sql, existing_relation, backup_relation, intermediate_relation) %}
        {% elif on_configuration_change == 'continue' %}
            {% set build_sql = '' %}
            {{ exceptions.warn("Configuration changes were identified and `on_configuration_change` was set to `continue` for `" ~ target_relation.render() ~ "`") }}
        {% elif on_configuration_change == 'fail' %}
            {{ exceptions.raise_fail_fast_error("Configuration changes were identified and `on_configuration_change` was set to `fail` for `" ~ target_relation.render() ~ "`") }}

        {% else %}
            -- this only happens if the user provides a value other than `apply`, 'skip', 'fail'
            {{ exceptions.raise_compiler_error("Unexpected configuration scenario") }}

        {% endif %}

    {% endif %}

    {% do return(build_sql) %}

{% endmacro %}


-- funcsign: (relation) -> string
{% macro materialized_view_execute_no_op(target_relation) %}
    {% do store_raw_result(
        name="main",
        message="skip " ~ target_relation,
        code="skip",
        rows_affected="-1"
    ) %}
{% endmacro %}


-- funcsign: (string, optional[relation], relation, list[hook]) -> string
{% macro materialized_view_execute_build_sql(build_sql, existing_relation, target_relation, post_hooks) %}

    -- `BEGIN` happens here:
    {{ run_hooks(pre_hooks, inside_transaction=True) }}

    {% set grant_config = config.get('grants') %}

    {% call statement(name="main") %}
        {{ build_sql }}
    {% endcall %}

    {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
    {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

    {% do persist_docs(target_relation, model) %}

    {{ run_hooks(post_hooks, inside_transaction=True) }}

    {{ adapter.commit() }}

{% endmacro %}
