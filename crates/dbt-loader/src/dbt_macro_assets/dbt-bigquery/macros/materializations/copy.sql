{% materialization copy, adapter='bigquery' -%}

  {# Setup #}
  {{ run_hooks(pre_hooks) }}

  {% set destination = this.incorporate(type='table') %}

  {# there can be several ref() or source() according to BQ copy API docs #}
  {# cycle over ref() and source() to create source tables array #}
  {% set source_array = [] %}
  {% for ref_table in model.refs %}
    {{ source_array.append(ref(ref_table.get('package'), ref_table.name, version=ref_table.get('version'))) }}
  {% endfor %}

  {% for src_table in model.sources %}
    {{ source_array.append(source(*src_table)) }}
  {% endfor %}

  {# Call adapter copy_table function #}
  {%- set result_str = adapter.copy_table(
      source_array,
      destination,
      config.get('copy_materialization', default = 'table')) -%}

  {{ store_result('main', response=result_str) }}

  {# Clean up #}
  {{ run_hooks(post_hooks) }}
  {%- do apply_grants(target_relation, grant_config) -%}
  {{ adapter.commit() }}

  {{ return({'relations': [destination]}) }}
{%- endmaterialization %}
