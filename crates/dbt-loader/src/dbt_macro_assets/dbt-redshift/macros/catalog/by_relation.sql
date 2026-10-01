{% macro redshift__get_catalog_relations(dbschema, relations) -%}


    {{ adapter.verify_database(dbschema.database) }}

    {#-- Compute a left-outer join in memory. Some Redshift queries are
      -- leader-only, and cannot be joined to other compute-based queries
      -- TODO: https://github.com/dbt-labs/fs/issues/6859#issue-3648357670 #}
    {% if redshift__use_show_apis() %}
        {% set catalog = _redshift__get_base_catalog_by_relation_show(dbschema.database, relations) %}
    {% else %}
        {#-- Compute a left-outer join in memory. Some Redshift queries are
          -- leader-only, and cannot be joined to other compute-based queries #}
        {% set catalog = _redshift__get_base_catalog_by_relation(dbschema.database, relations) %}
    {% endif %}

    {% set select_extended = redshift__can_select_from('svv_table_info') %}
    {% if select_extended %}
        {% set extended_catalog = _redshift__get_extended_catalog_by_relation(relations) %}
        {% set catalog = catalog.join(extended_catalog, ['table_schema', 'table_name']) %}
    {% else %}
        {{ redshift__no_svv_table_info_warning() }}
    {% endif %}

    {{ return(catalog) }}

{% endmacro %}


{# Standard Redshift catalog path: uses system catalog queries, which do not cover
   datashared objects across databases. #}
{% macro _redshift__get_base_catalog_by_relation(database, relations) -%}
    {%- call statement('base_catalog', fetch_result=True) -%}
        with
            late_binding as ({{ _redshift__get_late_binding_by_relation_sql(relations) }}),
            early_binding as ({{ _redshift__get_early_binding_by_relation_sql(database, relations) }}),
            unioned as (select * from early_binding union all select * from late_binding),
            table_owners as ({{ redshift__get_table_owners_sql() }})
        select '{{ database }}' as table_database, *
        from unioned
        join table_owners using (table_schema, table_name)
        order by "column_index"
    {%- endcall -%}
    {{ return(load_result('base_catalog').table) }}
{%- endmacro %}


{% macro _redshift__get_late_binding_by_relation_sql(relations) %}
    {{ redshift__get_late_binding_sql() }}
    where (
        {%- for relation in relations -%}
            (
                upper(table_schema) = upper('{{ relation.schema }}')
            and upper(table_name) = upper('{{ relation.identifier }}')
            )
            {%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
    )
{% endmacro %}


{% macro _redshift__get_early_binding_by_relation_sql(database, relations) %}
    {{ redshift__get_early_binding_sql(database) }}
    and (
        {%- for relation in relations -%}
            (
                upper(sch.nspname) = upper('{{ relation.schema }}')
            and upper(tbl.relname) = upper('{{ relation.identifier }}')
            )
            {%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
    )
{% endmacro %}


{% macro _redshift__get_base_catalog_by_relation_show(database, relations) -%}
    {% set schemas = [] %}
    {% for relation in relations %}
        {% if relation.schema not in schemas %}
            {% do schemas.append(relation.schema) %}
        {% endif %}
    {% endfor %}

    {% set columns_filter %}
        {%- for relation in relations -%}
            (schema_name = lower('{{ relation.schema }}') and table_name = lower('{{ relation.identifier }}'))
            {%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
    {% endset %}

    {{ return(_redshift__get_base_catalog_show(database, schemas, columns_filter)) }}
{%- endmacro %}


{% macro _redshift__get_extended_catalog_by_relation(relations) %}
    {%- call statement('extended_catalog', fetch_result=True) -%}
        {{ redshift__get_extended_catalog_sql() }}
        where (
            {%- for relation in relations -%}
                (
                    upper("schema") = upper('{{ relation.schema }}')
                and upper("table") = upper('{{ relation.identifier }}')
                )
            {%- if not loop.last %} or {% endif -%}
            {%- endfor -%}
        )
    {%- endcall -%}
    {{ return(load_result('extended_catalog').table) }}
{% endmacro %}
