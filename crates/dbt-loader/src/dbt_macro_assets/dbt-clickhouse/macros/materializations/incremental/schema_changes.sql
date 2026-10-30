{% macro clickhouse__apply_column_changes(column_changes, existing_relation, is_distributed=False) %}
    {{ log('Schema changes detected. Trying to apply the following changes: ' ~ column_changes) }}
    {%- set existing_local = none -%}
    {% if is_distributed %}
        {%- set local_suffix = adapter.get_clickhouse_local_suffix() -%}
        {%- set local_db_prefix = adapter.get_clickhouse_local_db_prefix() -%}
        {%- set existing_local = existing_relation.incorporate(path={"identifier": this.identifier + local_suffix, "schema": local_db_prefix + this.schema}) if existing_relation is not none else none -%}
    {% endif %}

    {% if column_changes.on_schema_change == 'append_new_columns' %}
        {% do clickhouse__add_columns(column_changes.columns_to_add, existing_relation, existing_local, is_distributed) %}

    {% elif column_changes.on_schema_change == 'sync_all_columns' %}
        {% do clickhouse__drop_columns(column_changes.columns_to_drop, existing_relation, existing_local, is_distributed) %}
        {% do clickhouse__add_columns(column_changes.columns_to_add, existing_relation, existing_local, is_distributed) %}
        {% do clickhouse__modify_columns(column_changes.columns_to_modify, existing_relation, existing_local, is_distributed) %}
    {% endif %}

{% endmacro %}

{% macro clickhouse__add_columns(columns, existing_relation, existing_local=none, is_distributed=False) %}
    {% for column in columns %}
        {% set codec = model['columns'].get(column.name, {}).get('codec') %}
        {% set alter_action -%}
            add column if not exists `{{ column.name }}` {{ column.data_type }} {{ codec_clause(codec) }}
        {%- endset %}
        {% do clickhouse__run_alter_table_command(alter_action, existing_relation, existing_local, is_distributed) %}
    {% endfor %}

{% endmacro %}

{% macro clickhouse__drop_columns(columns, existing_relation, existing_local=none, is_distributed=False) %}
    {% for column in columns %}
        {% set alter_action -%}
            drop column if exists `{{ column.name }}`
        {%- endset %}
        {% do clickhouse__run_alter_table_command(alter_action, existing_relation, existing_local, is_distributed) %}
    {% endfor %}

{% endmacro %}

{% macro clickhouse__modify_columns(columns, existing_relation, existing_local=none, is_distributed=False) %}
    {% for column in columns %}
        {% set alter_action -%}
            modify column if exists `{{ column.name }}` {{ column.data_type }}
        {%- endset %}
        {% do clickhouse__run_alter_table_command(alter_action, existing_relation, existing_local, is_distributed) %}
    {% endfor %}

{% endmacro %}

{% macro clickhouse__run_alter_table_command(alter_action, existing_relation, existing_local=none, is_distributed=False) %}
    {% if is_distributed %}
        {% call statement('alter_table') %}
            alter table {{ existing_local }} {{ on_cluster_clause(existing_relation) }} {{ alter_action }}
        {% endcall %}
        {% call statement('alter_table') %}
            alter table {{ existing_relation }} {{ on_cluster_clause(existing_relation) }} {{ alter_action }}
        {% endcall %}

    {% else %}
        {% call statement('alter_table') %}
            alter table {{ existing_relation }} {{ alter_action }}
        {% endcall %}
    {% endif %}

{% endmacro %}
