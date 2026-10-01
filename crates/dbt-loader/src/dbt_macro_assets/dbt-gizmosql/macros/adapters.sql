{#
    GizmoSQL is an Arrow Flight SQL server backed by DuckDB, so it inherits the
    `dbt-duckdb` macro package (see `get_adapter_prefixes` in dbt-jinja) and only
    overrides what differs for a *remote* DuckDB: the server cannot read files on
    the dbt client's machine, and grants are not supported.
#}

{#-- Seeds: `duckdb__load_csv_rows` fast-paths through `COPY ... FROM '<local
     seed file>'`, which only works when DuckDB runs in-process. The GizmoSQL
     server has no access to the client's filesystem, so use the default
     batched `insert ... values` loader instead. --#}
{% macro gizmosql__load_csv_rows(model, agate_table) %}
    {{ return(default__load_csv_rows(model, agate_table)) }}
{% endmacro %}

{% macro gizmosql__apply_grants(relation, grant_config, should_revoke=True) %}
    {#-- If grant_config is {} or None, this is a no-op --#}
    {% if grant_config %}
      {{ adapter.warn_once('Grants for relations are not supported by GizmoSQL') }}
    {% endif %}
{% endmacro %}

{#-- `duckdb__get_binding_char` reaches into a DuckDB-only adapter method. dbt v2
     inlines seed values as SQL literals, splitting on the `%s` placeholder. --#}
{% macro gizmosql__get_binding_char() %}
  {{ return('%s') }}
{% endmacro %}
