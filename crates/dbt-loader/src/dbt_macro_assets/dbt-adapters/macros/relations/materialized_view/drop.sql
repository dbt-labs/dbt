{# /*
This was already implemented. Instead of creating a new macro that aligns with the standard,
this was reused and the default was maintained. This gets called by `drop_relation`, which
actually executes the drop, and `get_drop_sql`, which returns the template.
*/ #}

-- funcsign: (relation) -> string
{% macro drop_materialized_view(relation) -%}
    {{- adapter.dispatch('drop_materialized_view', 'dbt')(relation) -}}
{%- endmacro %}


-- funcsign: (relation) -> string
{% macro default__drop_materialized_view(relation) -%}
    drop materialized view if exists {{ relation.render() }} cascade
{%- endmacro %}
