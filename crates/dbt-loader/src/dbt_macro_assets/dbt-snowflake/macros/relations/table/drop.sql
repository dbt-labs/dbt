{% macro snowflake__drop_table(relation) %}
    {#-- CASCADE is not supported in catalog-linked databases --#}
    {# DIVERGENCE BEGIN: core stashes this information on the relation; we cannot so we do some catalogs.yml hacks instead #}
    {# TODO: redesign the catalog_relation type to sidestep this whole problem #}
    {%- if dbt_version.startswith('2.') -%}
        {%- set catalog_relation = adapter.build_catalog_relation(relation.database) -%}
        {%- set _is_catalog_linked = snowflake__is_catalog_linked_database(relation=relation) or snowflake__is_catalog_linked_database(catalog_relation=catalog_relation) -%}
    {%- else -%}
        {%- set _is_catalog_linked = snowflake__is_catalog_linked_database(relation=relation) -%}
    {%- endif -%}
    {# DIVERGENCE END #}
    {% if _is_catalog_linked %}
        drop table if exists {{ relation }}
    {% else %}
        drop table if exists {{ relation }} cascade
    {% endif %}
{% endmacro %}
