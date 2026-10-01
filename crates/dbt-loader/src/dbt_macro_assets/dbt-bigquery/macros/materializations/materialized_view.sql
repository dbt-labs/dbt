{% materialization materialized_view, adapter='bigquery' -%}

    {% set relations = materialization_materialized_view_default() %}

    {% if config.get('grant_access_to') %}
      {% for grant_target_dict in config.get('grant_access_to') %}
        {% do adapter.grant_access_to(this, 'view', None, grant_target_dict) %}
      {% endfor %}
    {% endif %}

    {{ return(relations) }}

{%- endmaterialization %}
