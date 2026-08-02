{% macro sqlserver__create_view_as(relation, sql) -%}
    {#- Only guard against user-configured view materializations; this macro is also
        called for intermediate temp views during table/snapshot materializations,
        where query_options is intended for the *outer* DML and shouldn't trip a guard here. -#}
    {%- if config.get('materialized') == 'view' -%}
        {{ raise_if_query_options_set('view materializations (SQL Server does not accept OPTION on CREATE VIEW)') }}
    {%- endif -%}

    {%- if config.get('sql_header') -%}
        {{ exceptions.raise_compiler_error(
            "sql_header is not supported on SQL Server. "
            "CREATE VIEW must be the first statement in a batch, so sql_header cannot run in the same query. "
            "Use pre_hooks for pre-model SQL or query_options for session-level SET options (e.g. query_options={'NOCOUNT': 'ON'})."
        ) }}
    {%- endif -%}

    {{ get_use_database_sql(relation.database) }}
    {% set contract_config = config.get('contract') %}
    {% if contract_config.enforced %}
        {{ get_assert_columns_equivalent(sql) }}
    {%- endif %}

    {% set query %}
        CREATE OR ALTER VIEW {{ relation.include(database=False) }} AS {{ sql }};
    {% endset %}

    {{ get_use_database_sql(relation.database) }}
    EXEC('{{- escape_single_quotes(query) -}}')

{% endmacro %}
