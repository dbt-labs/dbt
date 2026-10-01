{% materialization view, adapter='fabricspark' -%}
    -- This uses the dbt implementation - https://github.com/dbt-labs/dbt-adapters/blob/60005a0a2bd33b61cb65a591bc1604b1b3fd25d5/dbt/include/global_project/macros/relations/view/replace.sql#L24
    {{ return(create_or_replace_view()) }}
{%- endmaterialization %}
