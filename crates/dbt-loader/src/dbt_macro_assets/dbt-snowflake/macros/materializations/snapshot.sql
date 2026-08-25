{% materialization snapshot, adapter='snowflake' %}
    {% set original_query_tag = set_query_tag() %}
    {% set relations = materialization_snapshot_default() %}

    {% do unset_query_tag(original_query_tag) %}

    {{ return(relations) }}
{% endmaterialization %}


{% macro snowflake__create_columns(relation, columns) %}
    {% if columns %}
        {% do alter_relation_add_remove_columns(relation, columns, []) %}
    {% endif %}
{% endmacro %}
