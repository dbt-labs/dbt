{% macro redis_adbc__catalog_query() -%}
select t.table_catalog as table_database, t.table_schema, t.table_name,
       t.table_type, coalesce(t.comment, '') as table_comment,
       '' as table_owner, c.column_name,
       cast(c.ordinal_position as decimal(38,0)) as column_index,
       c.driver_data_type as column_type, coalesce(c.comment, '') as column_comment
from information_schema.tables t
join information_schema.columns c
  on t.table_catalog = c.table_catalog and t.table_schema = c.table_schema and t.table_name = c.table_name
{%- endmacro %}

{% macro redis_adbc__get_catalog(information_schema, schemas) -%}
  {% set query %}
    {{ redis_adbc__catalog_query() }}
    where false
    {% for schema in schemas %}or t.table_schema = '{{ schema | replace("'", "''") }}'{% endfor %}
    order by t.table_schema, t.table_name, c.ordinal_position
  {% endset %}
  {{ return(run_query(query)) }}
{%- endmacro %}

{% macro redis_adbc__get_catalog_relations(information_schema, relations) -%}
  {% set query %}
    {{ redis_adbc__catalog_query() }}
    where false
    {% for relation in relations %}
      or (t.table_schema = '{{ relation.schema | replace("'", "''") }}'
      {% if relation.identifier %}and t.table_name = '{{ relation.identifier | replace("'", "''") }}'{% endif %})
    {% endfor %}
    order by t.table_schema, t.table_name, c.ordinal_position
  {% endset %}
  {{ return(run_query(query)) }}
{%- endmacro %}
