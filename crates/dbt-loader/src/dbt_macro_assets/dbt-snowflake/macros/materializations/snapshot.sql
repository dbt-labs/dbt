{% materialization snapshot, adapter='snowflake' %}
    {% set original_query_tag = set_query_tag() %}
    {% set relations = materialization_snapshot_default() %}

    {% do unset_query_tag(original_query_tag) %}

    {{ return(relations) }}
{% endmaterialization %}
