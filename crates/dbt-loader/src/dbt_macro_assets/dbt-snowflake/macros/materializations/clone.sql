{% macro snowflake__can_clone_table() %}
    {{ return(True) }}
{% endmacro %}

{% macro snowflake__create_or_replace_clone(this_relation, defer_relation) %}
    create or replace
      {{ "transient" if config.get("transient", true) }}
      table {{ this_relation }}
      clone {{ defer_relation }}
      {{ "copy grants" if config.get("copy_grants", false) }}
      {{ "copy tags" if config.get("copy_tags", false) }}
{% endmacro %}
