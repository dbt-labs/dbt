-- funcsign: (string, string, string, string) -> string
{% macro get_tables_by_pattern_sql(schema_pattern, table_pattern, exclude='', database=target.database) %}
    {{ return(adapter.dispatch('get_tables_by_pattern_sql')
        (schema_pattern, table_pattern, exclude, database)) }}
{% endmacro %}

-- funcsign: (string, string, string, string) -> string
{% macro default__get_tables_by_pattern_sql(schema_pattern, table_pattern, exclude='', database=target.database) %}

        select distinct
            table_schema as {{ adapter.quote('table_schema') }},
            table_name as {{ adapter.quote('table_name') }},
            {{ adapter.dispatch('get_table_types_sql')() }}
        from {{ database }}.information_schema.tables
        where table_schema ilike '{{ schema_pattern }}'
        and table_name ilike '{{ table_pattern }}'
        and table_name not ilike '{{ exclude }}'

{% endmacro %}

-- funcsign: (string, string, string, string) -> string
{% macro redshift__get_tables_by_pattern_sql(schema_pattern, table_pattern, exclude='', database=target.database) %}

    {% set sql %}
        select distinct
            table_schema as {{ adapter.quote('table_schema') }},
            table_name as {{ adapter.quote('table_name') }},
            {{ adapter.dispatch('get_table_types_sql')() }}
        from "{{ database }}"."information_schema"."tables"
        where table_schema ilike '{{ schema_pattern }}'
        and table_name ilike '{{ table_pattern }}'
        and table_name not ilike '{{ exclude }}'
        union all
        select distinct
            schemaname as {{ adapter.quote('table_schema') }},
            tablename as {{ adapter.quote('table_name') }},
            'external' as {{ adapter.quote('table_type') }}
        from svv_external_tables
        where redshift_database_name = '{{ database }}'
        and schemaname ilike '{{ schema_pattern }}'
        and table_name ilike '{{ table_pattern }}'
        and table_name not ilike '{{ exclude }}'
    {% endset %}

    {{ return(sql) }}
{% endmacro %}

-- funcsign: (string, string, string, string) -> string
{% macro bigquery__get_tables_by_pattern_sql(schema_pattern, table_pattern, exclude='', database=target.database) %}

    {% if '%' in schema_pattern %}
        {% set schemata=_bigquery__get_matching_schemata(schema_pattern, database) %}
    {% else %}
        {% set schemata=[schema_pattern] %}
    {% endif %}

    {% set sql %}
        {% for schema in schemata %}
            select distinct
                table_schema,
                table_name,
                {{ adapter.dispatch('get_table_types_sql')() }}

            from {{ adapter.quote(database) }}.{{ schema }}.INFORMATION_SCHEMA.TABLES
            where lower(table_name) like lower ('{{ table_pattern }}')
                and lower(table_name) not like lower ('{{ exclude }}')

            {% if not loop.last %} union all {% endif %}

        {% endfor %}
    {% endset %}

    {{ return(sql) }}

{% endmacro %}

-- funcsign: (string, string) -> string
{% macro _bigquery__get_matching_schemata(schema_pattern, database) %}
    {% if execute %}

        {% set sql %}
        select schema_name from {{ adapter.quote(database) }}.INFORMATION_SCHEMA.SCHEMATA
        where lower(schema_name) like lower('{{ schema_pattern }}')
        {% endset %}

        {% set results=run_query(sql) %}

        {% set schemata=results.columns['schema_name'].values() %}

        {{ return(schemata) }}

    {% else %}

        {{ return([]) }}

    {% endif %}


{% endmacro %}
