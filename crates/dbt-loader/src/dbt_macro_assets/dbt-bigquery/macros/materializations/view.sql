
{% macro bigquery__handle_existing_table(full_refresh, old_relation) %}
    {%- if full_refresh -%}
      {{ adapter.drop_relation(old_relation) }}
    {%- else -%}
      {{ exceptions.relation_wrong_type(old_relation, 'view') }}
    {%- endif -%}
{% endmacro %}


{% materialization view, adapter='bigquery' -%}
    -- grab current tables grants config for comparision later on
    {% set grant_config = config.get('grants') %}

    {% set to_return = bigquery__create_or_replace_view() %}

    {% set target_relation = this.incorporate(type='view') %}

    {% do persist_docs(target_relation, model) %}

    {% if config.get('grant_access_to') %}
      {% for grant_target_dict in config.get('grant_access_to') %}
        {% do adapter.grant_access_to(this, 'view', None, grant_target_dict) %}
      {% endfor %}
    {% endif %}

    {% do return(to_return) %}

{%- endmaterialization %}
