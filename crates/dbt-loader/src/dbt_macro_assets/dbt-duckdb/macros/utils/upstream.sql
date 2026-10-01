{%- macro register_upstream_external_models() -%}
{% if execute %}
{% set upstream_nodes = {} %}
{% set upstream_schemas = {} %}
{% for node in selected_resources %}
  {% if node not in graph['nodes'] %}{% continue %}{% endif %}
  {% for upstream_node in graph['nodes'][node]['depends_on']['nodes'] %}
    {% if upstream_node not in upstream_nodes and upstream_node not in selected_resources %}
      {% do upstream_nodes.update({upstream_node: None}) %}
      {% set upstream = graph['nodes'].get(upstream_node) %}
      {% if upstream
         and upstream.resource_type in ('model', 'seed')
         and upstream.config.materialized=='external'
      %}
        {%- set upstream_rel = api.Relation.create(
          database=upstream['database'],
          schema=upstream['schema'],
          identifier=upstream['alias']
        ) -%}
        {%- set location = upstream.config.get('location', external_location(upstream_rel, upstream.config)) -%}
        {%- set rendered_options = render_write_options(upstream.config) -%}
        {%- set upstream_location = adapter.external_read_location(location, rendered_options) -%}
        {% if upstream_rel.schema not in upstream_schemas %}
          {% call statement('main', language='sql') -%}
            create schema if not exists {{ upstream_rel.without_identifier() }}
          {%- endcall %}
          {% do upstream_schemas.update({upstream_rel.schema: None}) %}
        {% endif %}
        {% call statement('main', language='sql') -%}
          create or replace view {{ upstream_rel }} as (
            select * from '{{ upstream_location }}'
          );
        {%- endcall %}
      {%- endif %}
    {% endif %}
  {% endfor %}
{% endfor %}
{% if upstream_schemas %}
  {% do adapter.commit() %}
{% endif %}
{% endif %}
{%- endmacro -%}
