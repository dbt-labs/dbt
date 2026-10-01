{% macro exasol__alter_relation_comment(relation, relation_comment) -%}
    {#- Exasol comments on views can only be set inside the CREATE VIEW DDL, not
        via COMMENT ON (see https://docs.exasol.com/db/latest/sql/comment.htm),
        so skip views here; table relation docs use COMMENT ON. -#}
    {%- if not relation.is_view -%}
        {%- set comment = relation_comment | replace("'", "''") -%}
        comment on {{ relation.type }} {{ relation }} is '{{ comment }}'
    {%- else -%}
        {#- no-op for views; emit a harmless statement so run_query has valid SQL -#}
        select 1
    {%- endif -%}
{%- endmacro %}


{% macro exasol__alter_column_comment(relation, column_dict) -%}
    {%- if not relation.is_view -%}
        {%- set comments = [] -%}
        {%- for col_name, col in column_dict.items() -%}
            {%- if col.description -%}
                {%- set c = col.description | replace("'", "''") -%}
                {%- do comments.append(adapter.quote(col.name | upper) ~ " is '" ~ c ~ "'") -%}
            {%- endif -%}
        {%- endfor -%}
        {%- if comments | length > 0 -%}
            comment on {{ relation.type }} {{ relation }} ({{ comments | join(', ') }})
        {%- else -%}
            select 1
        {%- endif -%}
    {%- else -%}
        select 1
    {%- endif -%}
{%- endmacro %}
