{% macro assert_refresh_schedules(results) %}
  {% if not execute %}{{ return('') }}{% endif %}
  {% set checked = [] %}
  {% for result in results %}
    {% if result.node and result.node.name in ['scheduled_mv', 'scheduled_st'] %}
      {% set name = result.node.name %}
      {% if result.status != 'success' %}
        {{ exceptions.raise_compiler_error(name ~ ' did not finish successfully: ' ~ result.status) }}
      {% endif %}
      {% set relation = api.Relation.create(database=result.node.database, schema=result.node.schema, identifier=result.node.alias) %}
      {% set description = run_query('DESCRIBE EXTENDED ' ~ relation) %}
      {% set schedules = [] %}
      {% for row in description.rows %}
        {% if row[0] == 'Refresh Schedule' %}
          {% do schedules.append(row[1]) %}
        {% endif %}
      {% endfor %}
      {% set expected = var('expected_mv') if name == 'scheduled_mv' else var('expected_st') %}
      {% if schedules | length != 1 or schedules[0] not in expected %}
        {{ exceptions.raise_compiler_error(name ~ ': expected refresh schedule in ' ~ expected ~ ', got ' ~ schedules) }}
      {% endif %}
      {% if name == 'scheduled_st' %}
        {% set properties = run_query('SHOW TBLPROPERTIES ' ~ relation) %}
        {% set markers = [] %}
        {% for row in properties.rows %}
          {% if row[0] == 'refresh_test_marker' %}{% do markers.append(row[1]) %}{% endif %}
        {% endfor %}
        {% if markers != [var('st_marker')] %}
          {{ exceptions.raise_compiler_error('Streaming table property was not applied: ' ~ markers) }}
        {% endif %}
      {% endif %}
      {% do checked.append(name) %}
    {% endif %}
  {% endfor %}
  {% if checked | sort != ['scheduled_mv', 'scheduled_st'] %}
    {{ exceptions.raise_compiler_error('Expected results for both scheduled models, got ' ~ checked) }}
  {% endif %}
{% endmacro %}
