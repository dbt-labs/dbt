
{% macro bigquery__create_csv_table(model, agate_table) %}
    {% if (agate_table.rows | length) == 0 %}
        {# `bigquery__load_csv_rows` (which normally creates the table via
           `adapter.load_dataframe`) is skipped when the agate has no rows,
           so we have to emit a real CREATE TABLE here. #}
        {{ return(dbt.default__create_csv_table(model, agate_table)) }}
    {% endif %}
    -- no-op
{% endmacro %}

{% macro bigquery__reset_csv_table(model, full_refresh, old_relation, agate_table) %}
    {{ adapter.drop_relation(old_relation) }}
    {% if (agate_table.rows | length) == 0 %}
        {{ return(dbt.default__create_csv_table(model, agate_table)) }}
    {% endif %}
{% endmacro %}

{% macro bigquery__load_csv_rows(model, agate_table) %}

  {# DIVERGENCE BEGIN: Fusion's adapter.load_dataframe takes an extra `file_path`
     argument so it can read the CSV from disk; the upstream Python dbt-bigquery
     adapter's load_dataframe only accepts 6 args. Gate on `dbt_version` (Fusion
     reports a `2.x` version, Python dbt-core reports `1.x`) so the same macro
     works under both runtimes (Fusion parser + Python adapter execution path
     included). #}
  {%- set column_override = model['config'].get('column_types', {}) -%}
  {%- set delimiter = model['config'].get('delimiter', ',') -%}
  {% if dbt_version.startswith('2.') %}
    {{ adapter.load_dataframe(
        model['database'],
        model['schema'],
        model['alias'],
        model['project_root'] | string ~ model['original_file_path'] | string,
        agate_table,
        column_override,
        delimiter,
    ) }}
  {% else %}
    {{ adapter.load_dataframe(
        model['database'],
        model['schema'],
        model['alias'],
        agate_table,
        column_override,
        delimiter,
    ) }}
  {% endif %}
  {# DIVERGENCE END #}

  {% call statement() %}
    alter table {{ this.render() }} set {{ bigquery_table_options(config, model) }}
  {% endcall %}

  {% if config.persist_relation_docs() and model.description %}

  	{{ adapter.update_table_description(model['database'], model['schema'], model['alias'], model['description']) }}
  {% endif %}
{% endmacro %}
