{% macro redshift__get_create_materialized_view_as_sql(relation, sql) %}

    {%- set materialized_view = relation.from_config(config.model) -%}

    create materialized view {{ materialized_view.path }}
        backup {% if materialized_view.backup %}yes{% else %}no{% endif %}
        diststyle {{ materialized_view.dist.diststyle }}
        {% if materialized_view.dist.distkey %}distkey ({{ materialized_view.dist.distkey }}){% endif %}
        {% if materialized_view.sort.sortkey %}sortkey ({{ ','.join(materialized_view.sort.sortkey) }}){% endif %}
        auto refresh {% if materialized_view.autorefresh %}yes{% else %}no{% endif %}
    as (
        {# DIVERGENCE: compiled sql can be used directly instead of materialized_view.query (from RedshiftMaterializedViewConfig in v2) #}
        {{ sql }}
    )

{% endmacro %}
