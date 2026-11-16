{% macro _fq() -%}
  {{ target.database }}.{{ target.schema }}
{%- endmacro %}

{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% do adapter.execute('DROP DYNAMIC TABLE IF EXISTS ' ~ _fq() ~ '.ci_attrs') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.orders (order_id int, amount number(10,2)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.orders VALUES (1, 10.00), (2, 20.00)') %}
  {% do adapter.execute('CREATE OR REPLACE TAG ' ~ _fq() ~ '.mytag') %}
  {% do adapter.execute('CREATE OR REPLACE ROW ACCESS POLICY ' ~ _fq() ~ '.rap AS (order_id number) RETURNS BOOLEAN -> true') %}
{% endmacro %}

{# A grant dbt doesn't manage, for COPY GRANTS to carry across a replace. #}
{% macro grant_out_of_band() %}
  {% do adapter.execute('GRANT SELECT ON DYNAMIC TABLE ' ~ _fq() ~ '.ci_attrs TO ROLE PUBLIC') %}
{% endmacro %}

{#-
    Each expectation is printed as `[PASS]`/`[FAIL]`; the stdout goldie pins the outcome.
-#}
{% macro _check(label, ok) %}
  {% do log(("[PASS] " if ok else "[FAIL] ") ~ label, info=True) %}
{% endmacro %}

{% macro _scalar(sql) %}
  {% set t = run_query(sql) %}
  {% if t.rows | length == 0 %}{% do return(none) %}{% endif %}
  {% do return(t.rows[0][0]) %}
{% endmacro %}

{% macro verify(stage, cluster_key) %}
  {% set table = _fq() ~ '.ci_attrs' %}
  {% do run_query("show dynamic tables like 'CI_ATTRS' in schema " ~ _fq()) %}
  {% set dt = run_query('select "refresh_mode", "cluster_by", "initialization_warehouse" from table(result_scan(last_query_id()))').rows[0] %}
  {% set comment = _scalar("select comment from " ~ target.database ~ ".information_schema.tables where table_schema = upper('" ~ target.schema ~ "') and table_name = 'CI_ATTRS'") %}
  {% set tag = _scalar("select system$get_tag('" ~ _fq() ~ ".mytag', '" ~ table ~ "', 'table')") %}
  {% set policies = run_query("select policy_name from table(" ~ target.database ~ ".information_schema.policy_references(ref_entity_name => '" ~ table ~ "', ref_entity_domain => 'table'))").rows | map(attribute=0) | list %}
  {% do run_query("show grants on dynamic table " ~ table) %}
  {% set public_select = _scalar('select count(*) from table(result_scan(last_query_id())) where "privilege" = \'SELECT\' and "grantee_name" = \'PUBLIC\'') %}
  {{ _check(stage ~ ": refresh_mode is CUSTOM_INCREMENTAL", dt[0] == 'CUSTOM_INCREMENTAL') }}
  {{ _check(stage ~ ": cluster by " ~ cluster_key, (dt[1] | lower) == 'linear(' ~ cluster_key ~ ')') }}
  {{ _check(stage ~ ": initialization warehouse", dt[2] == 'FUSION_ADAPTER_TESTING') }}
  {{ _check(stage ~ ": tag mytag = v1", tag == 'v1') }}
  {{ _check(stage ~ ": row access policy RAP", 'RAP' in policies) }}
  {{ _check(stage ~ ": comment from persist_docs", comment == 'orders merged from their changes') }}
  {{ _check(stage ~ ": out-of-band SELECT grant to PUBLIC", public_select | int == 1) }}
{% endmacro %}
