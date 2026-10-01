{% materialization view, adapter='snowflake' -%}

    {% set original_query_tag = set_query_tag() %}
    {% set to_return = snowflake__create_or_replace_view() %}

    {% set target_relation = this.incorporate(type='view') %}

    {% do persist_docs(target_relation, model, for_columns=false) %}

    {% do unset_query_tag(original_query_tag) %}

    {% do return(to_return) %}

{%- endmaterialization %}
