{% macro wap_fixture_relation(identifier) %}
  {% if target.type != 'snowflake' %}
    {{ exceptions.raise_compiler_error('This acceptance fixture requires a Snowflake target') }}
  {% endif %}
  {% set public = ref('orders') %}
  {{ return(public.incorporate(path={'identifier': identifier})) }}
{% endmacro %}

{% macro wap_fixture_assert(identifier, present=true, expected_ids=none, expected_transient=none) %}
  {% set relation = wap_fixture_relation(identifier) %}
  {% set actual = adapter.get_relation(database=relation.database, schema=relation.schema, identifier=relation.identifier) %}
  {% if (actual is not none) != present %}
    {{ exceptions.raise_compiler_error('Unexpected presence for ' ~ relation ~ ': expected present=' ~ present) }}
  {% endif %}
  {% if present and expected_transient is not none %}
    {% set sql = "show tables in schema " ~ relation.include(identifier=false) ~ " starts with '" ~ identifier.replace("'", "''") ~ "' limit 2" %}
    {% set result = run_query(sql) %}
    {% set kinds = [] %}
    {% for row in result.rows %}
      {% if row['name'] == identifier %}
        {% do kinds.append(row['kind']) %}
      {% endif %}
    {% endfor %}
    {% set expected_kind = 'TRANSIENT' if expected_transient else 'TABLE' %}
    {% if kinds != [expected_kind] %}
      {{ exceptions.raise_compiler_error('Unexpected lifecycle for ' ~ relation ~ ': ' ~ kinds ~ ', expected ' ~ expected_kind) }}
    {% endif %}
  {% endif %}
  {% if present and expected_ids is not none %}
    {% set result = run_query('select id from ' ~ relation ~ ' order by id') %}
    {% set actual_ids = [] %}
    {% for row in result.rows %}
      {% do actual_ids.append(row[0] | int) %}
    {% endfor %}
    {% if actual_ids != expected_ids %}
      {{ exceptions.raise_compiler_error('Unexpected rows in ' ~ relation ~ ': ' ~ actual_ids ~ ', expected ' ~ expected_ids) }}
    {% endif %}
  {% endif %}
{% endmacro %}

{% macro wap_fixture_prepare() %}
  {% for model_name, sentinel_id in [('orders', 999), ('downstream', 777)] %}
    {% set relation = ref(model_name) %}
    {% do wap_fixture_assert(relation.identifier, present=false) %}
    {% do run_query('create ' ~ ('transient ' if var('transient', false) else '') ~ 'table ' ~ relation ~ ' as select ' ~ sentinel_id ~ '::integer as id') %}
  {% endfor %}
{% endmacro %}

{% macro wap_fixture_cleanup(identifiers) %}
  {% for identifier in identifiers %}
    {% if not identifier.startswith(var('prefix') ~ '_') and not identifier.startswith('__DBT_WAP_') %}
      {{ exceptions.raise_compiler_error('Refusing unexpected cleanup identifier: ' ~ identifier) }}
    {% endif %}
    {% set relation = wap_fixture_relation(identifier) %}
    {% do run_query('drop table if exists ' ~ relation) %}
  {% endfor %}
{% endmacro %}
