{% macro snowflake__can_clone_table() %}
    {{ return(True) }}
{% endmacro %}

{# DIVERGENCE BEGIN: the clone target carries the source's table format, so the DDL, persist_docs and the relation cache all see it (dbt-labs/dbt-adapters#1767). #}
{% macro snowflake__get_clone_target_relation(this_relation, source_relation) %}
    {{ return(source_relation.incorporate(
        path={'database': this_relation.database, 'schema': this_relation.schema, 'identifier': this_relation.identifier},
        type='table'
    ).quote(
        database=this_relation.quote_policy.database,
        schema=this_relation.quote_policy.schema,
        identifier=this_relation.quote_policy.identifier
    )) }}
{% endmacro %}

{% macro snowflake__clone_target_drop_reason(existing_relation, target_relation) %}
    {% if not existing_relation.is_table %}
        {{ return("it is of type " ~ existing_relation.type) }}
    {% elif existing_relation.is_iceberg_format != target_relation.is_iceberg_format %}
        {{ return("it is " ~ ("an Iceberg table" if existing_relation.is_iceberg_format else "a regular table")
            ~ " and the clone source is " ~ ("an Iceberg table" if target_relation.is_iceberg_format else "a regular table")) }}
    {% elif target_relation.needs_to_drop(existing_relation) %}
        {{ return("it cannot be replaced in place by the clone") }}
    {% endif %}
    {{ return(none) }}
{% endmacro %}

{% macro snowflake__create_or_replace_clone(this_relation, defer_relation) %}
    create or replace
      {% if this_relation.is_iceberg_format -%}
        iceberg
      {%- else -%}
        {{ "transient" if config.get("transient", true) }}
      {%- endif %}
      table {{ this_relation }}
      clone {{ defer_relation }}
      {{ "copy grants" if config.get("copy_grants", false) }}
      {{ "copy tags" if config.get("copy_tags", false) }}
{% endmacro %}
{# DIVERGENCE END #}
