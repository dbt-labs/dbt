{% macro clickhouse__get_or_create_relation(database, schema, identifier, type) %}
  {%- set target_relation = adapter.get_relation(database=database, schema=schema, identifier=identifier) %}
  {% if target_relation %}
    {% do return([true, target_relation]) %}
  {% endif %}

  {%- set can_exchange = adapter.can_exchange(schema, type) %}
  {%- set should_on_cluster = adapter.should_on_cluster(config.get('materialized'), engine_clause()) %}
  {%- set new_relation = api.Relation.create(
      database=None,
      schema=schema,
      identifier=identifier,
      type=type,
      can_exchange=can_exchange,
      can_on_cluster=should_on_cluster
  ) -%}
  {% do return([false, new_relation]) %}
{% endmacro %}

{% macro clickhouse__get_database(database) %}
    {% call statement('get_database', fetch_result=True) %}
        select name, engine, comment
        from system.databases
        where name = '{{ database }}'
   {% endcall %}
   {% do return(load_result('get_database').table) %}
{% endmacro %}