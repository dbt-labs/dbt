-- funcsign: (relation, relation) -> string
{%- macro get_rename_sql(relation, new_name) -%}
    {{- log('Applying RENAME to: ' ~ relation) -}}
    {{- adapter.dispatch('get_rename_sql', 'dbt')(relation, new_name) -}}
{%- endmacro -%}


-- funcsign: (relation, relation) -> string
{%- macro default__get_rename_sql(relation, new_name) -%}

    {%- if relation.is_view -%}
        {{ get_rename_view_sql(relation, new_name) }}

    {%- elif relation.is_table -%}
        {{ get_rename_table_sql(relation, new_name) }}

    {%- elif relation.is_materialized_view -%}
        {{ get_rename_materialized_view_sql(relation, new_name) }}

    {%- else -%}
        {{- exceptions.raise_compiler_error("`get_rename_sql` has not been implemented for: " ~ relation.type ) -}}

    {%- endif -%}

{%- endmacro -%}


-- funcsign: (relation, relation) -> string
{% macro rename_relation(from_relation, to_relation) -%}
  {{ return(adapter.dispatch('rename_relation', 'dbt')(from_relation, to_relation)) }}
{% endmacro %}

-- funcsign: (relation, relation) -> string
{% macro default__rename_relation(from_relation, to_relation) -%}
  {% set target_name = adapter.quote_as_configured(to_relation.identifier, 'identifier') %}
  {% call statement('rename_relation') -%}
    alter table {{ from_relation.render() }} rename to {{ target_name }}
  {%- endcall %}
{% endmacro %}
