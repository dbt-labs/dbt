{% macro snowflake__can_clone_table() %}
    {{ return(True) }}
{% endmacro %}

{% macro snowflake__create_or_replace_clone(this_relation, defer_relation) %}
    {%- set is_iceberg = snowflake__is_iceberg_table(defer_relation) -%}
    create or replace
      {% if is_iceberg -%}
        iceberg
      {%- else -%}
        {{ "transient" if config.get("transient", true) }}
      {%- endif %}
      table {{ this_relation }}
      clone {{ defer_relation }}
      {{ "copy grants" if config.get("copy_grants", false) }}
      {{ "copy tags" if config.get("copy_tags", false) }}
{% endmacro %}

{% macro snowflake__is_iceberg_table(relation) %}
    {%- set source_relation = load_cached_relation(relation) -%}
    {%- if source_relation is not none -%}
        {{ return(source_relation.is_iceberg_format) }}
    {%- endif -%}

    {{ return(false) }}
{% endmacro %}
