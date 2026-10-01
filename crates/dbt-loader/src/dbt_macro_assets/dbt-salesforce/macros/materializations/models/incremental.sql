{% materialization incremental, adapter='salesforce', supported_languages=['sql'] %}

  {%- set dlo_api_name = model['name'] -%}
  {%- set identifier = model['alias'] -%}
  {%- set primary_key = salesforce__get_primary_key() -%}
  {%- set category = salesforce__get_category() -%}
  {%- set write_mode = salesforce__get_write_mode() -%}

  {%- set target_relation = api.Relation.create(
	identifier=identifier,
	schema=schema,
	database=database,
	type='table'
   ) -%}

  {# Though adapter.execute supports passing the Arrow ADBC Statement options, it is not recommended to use in any user defined macros. #}
  {% do adapter.execute(
    sql=compiled_code,
    auto_begin=False,
    fetch=False,
    limit=None,
    options={
      "adbc.salesforce.dc.dlo.primary_key": primary_key,
      "adbc.salesforce.dc.dlo.category": category,
      "adbc.salesforce.dc.dlo.target_dlo": identifier,
      "adbc.salesforce.dc.dlo.materialized": "table",
      "adbc.salesforce.dc.dlo.write_mode": write_mode
    })%}

  {{ return({'relations': [target_relation]}) }}

{% endmaterialization %}
