{% macro assert_cloned_views() %}
  {% for name in ['governed_mv', 'filtered_st', 'masked_st'] %}
    {% set relation = adapter.get_relation(database=target.database, schema=target.schema, identifier=name) %}
    {% if relation is none or not relation.is_view %}
      {{ exceptions.raise_compiler_error(name ~ ' must be cloned as a view') }}
    {% endif %}
    {% for suffix in ['', '_prod'] %}
      {% set relation = api.Relation.create(database=target.database, schema=target.schema, identifier=name ~ suffix) %}
      {% if name == 'masked_st' %}
        {% set result = run_query('select id, password from ' ~ relation ~ ' order by id') %}
        {% if result.rows | length != 1 or result.rows[0][0] != 'abc123' or result.rows[0][1] != '*****' %}
          {{ exceptions.raise_compiler_error('Clone and source must preserve the column mask: ' ~ relation) }}
        {% endif %}
      {% else %}
        {% set result = run_query('select cast(id as string), region from ' ~ relation ~ ' order by id') %}
        {% if result.rows | length != 1 or result.rows[0][0] != '1' or result.rows[0][1] != 'a' %}
          {{ exceptions.raise_compiler_error('Clone and source must preserve the row filter: ' ~ relation) }}
        {% endif %}
      {% endif %}
    {% endfor %}
  {% endfor %}
{% endmacro %}

{% macro replace_clones_with_sentinels() %}
  {% for name in ['governed_mv', 'filtered_st', 'masked_st'] %}
    {% set relation = api.Relation.create(database=target.database, schema=target.schema, identifier=name) %}
    {% do run_query('create or replace view ' ~ relation ~ " as select 'sentinel' as id") %}
  {% endfor %}
{% endmacro %}
