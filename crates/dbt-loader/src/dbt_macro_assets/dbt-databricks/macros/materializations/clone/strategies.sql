{% macro databricks__create_or_replace_clone(this_relation, defer_relation) %}
    create or replace
    table {{ this_relation.render() }}
    shallow clone {{ defer_relation.render() }}
{% endmacro %}

{#- DIVERGENCE BEGIN: v2 does not store databricks_table_type, so we cannot
    check against it here. -#}
{% macro clone_requires_drop(existing_relation) %}
    {{ return(existing_relation is not none and not (existing_relation.is_table and existing_relation.is_shallow_clone)) }}
{% endmacro %}
{#- DIVERGENCE END -#}

{% macro clone_source_has_policies(relation) %}
  {#- UC rejects a shallow clone of a table with a row filter or column mask -#}
  {%- if relation.is_hive_metastore() -%}
    {{ return(False) }}
  {#- Mirrors adapter.is_describe_as_json_supported, which isn't callable from Jinja -#}
  {%- elif adapter.behavior.use_describe_as_json_for_relation_metadata.no_warn
        and adapter.has_dbr_capability('describe_table_extended_as_json') -%}
    {%- set described = describe_table_extended_as_json(relation) -%}
    {%- set metadata = fromjson(described.rows[0]['json_metadata']) -%}
    {{ return(true if metadata.get('row_filter') or metadata.get('column_masks') else false) }}
  {%- else -%}
    {%- call statement('clone_source_policies', fetch_result=True) -%}
      {{ clone_source_policies_sql(relation) }}
    {%- endcall -%}
    {{ return(load_result('clone_source_policies').table.rows | length > 0) }}
  {%- endif -%}
{% endmacro %}

{% macro clone_source_policies_sql(relation) %}
  SELECT 1
  FROM `{{ relation.database }}`.`information_schema`.`row_filters`
  WHERE table_catalog = '{{ relation.database | lower }}'
    AND table_schema = '{{ relation.schema | lower }}'
    AND table_name = '{{ relation.identifier | lower }}'
  UNION ALL
  SELECT 1
  FROM `{{ relation.database }}`.`information_schema`.`column_masks`
  WHERE table_catalog = '{{ relation.database | lower }}'
    AND table_schema = '{{ relation.schema | lower }}'
    AND table_name = '{{ relation.identifier | lower }}'
{% endmacro %}

{% macro create_or_replace_clone_external(this_relation, defer_relation) %}

    {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

    create or replace
    table {{ this_relation.render() }}
    shallow clone {{ defer_relation.render() }}
    {{ location_clause(catalog_relation) }}

{% endmacro %}
