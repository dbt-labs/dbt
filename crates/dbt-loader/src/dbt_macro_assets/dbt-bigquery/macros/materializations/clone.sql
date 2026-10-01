{% macro bigquery__can_clone_table() %}
    {{ return(True) }}
{% endmacro %}

{% macro bigquery__create_or_replace_clone(this_relation, defer_relation) %}
    drop table if exists {{ this_relation }};
    create or replace
      table {{ this_relation }}
      clone {{ defer_relation }};
{% endmacro %}
