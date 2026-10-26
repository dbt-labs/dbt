{% set columns = adapter.get_columns_in_relation(ref('a')) %}

{#-- dbt#16472: hive_metastore takes the plain DESCRIBE path instead of AS JSON --#}
{% if target.type == 'databricks' %}
  {% set hms_columns = adapter.get_columns_in_relation(api.Relation.create('hive_metastore', target.schema, 'a')) %}
{% endif %}

select 1 as one
