{%- macro fabricspark__create_table_as(temporary, relation, compiled_code, language='sql') -%}
  {%- if language == 'sql' -%}
    {%- if temporary -%}
      {{ create_temporary_view(relation, compiled_code) }}
    {%- else -%}      
      {% if config.get('file_format') == 'delta' or relation.is_delta %}
        create or replace table {{ relation }}
      {% else %}
        create table {{ relation }}
      {% endif %}
      {%- set contract_config = config.get('contract') -%}
      {%- if contract_config.enforced -%}
        {{ get_assert_columns_equivalent(compiled_code) }}
        {%- set compiled_code = get_select_subquery(compiled_code) %}
      {% endif %}
      {{ file_format_clause() }}
      {{ options_clause() }}
      {{ tblproperties_clause() }}
      {{ partition_cols(label="partitioned by") }}
      {{ clustered_cols(label="clustered by") }}
      {{ location_clause() }}
      {{ comment_clause() }}

      as
      {{ compiled_code }}
    {%- endif -%}
  {%- elif language == 'python' -%}
    {{ exceptions.raise_compiler_error("Python models are not supported in Fabric Spark") }}
  {%- endif -%}
{%- endmacro -%}

{% macro file_format_clause() %}
  {{ return(adapter.dispatch('file_format_clause', 'dbt')()) }}
{%- endmacro -%}

{% macro fabricspark__file_format_clause() %}
  {%- set file_format = config.get('file_format') -%}
  {%- if file_format is not none and file_format != 'delta' %}
    using {{ file_format }}
  {%- endif %}
{%- endmacro -%}
W
{% macro tblproperties_clause() %}
  {{ return(adapter.dispatch('tblproperties_clause', 'dbt')()) }}
{%- endmacro -%}

{% macro fabricspark__tblproperties_clause() -%}
  {%- set tblproperties = config.get('tblproperties') -%}
  {%- if tblproperties is not none %}
    tblproperties (
      {%- for prop in tblproperties -%}
      '{{ prop }}' = '{{ tblproperties[prop] }}' {% if not loop.last %}, {% endif %}
      {%- endfor %}
    )
  {%- endif %}
{%- endmacro -%}

{% macro location_clause() %}
  {{ return(adapter.dispatch('location_clause', 'dbt')()) }}
{%- endmacro -%}

{% macro fabricspark__location_clause() %}
  {%- set location_root = config.get('location_root', validator=validation.any[basestring]) -%}
  {%- set identifier = model['alias'] -%}
  {%- if location_root is not none %}
    location '{{ location_root }}/{{ identifier }}'
  {%- endif %}
{%- endmacro -%}


{% macro options_clause() -%}
  {{ return(adapter.dispatch('options_clause', 'dbt')()) }}
{%- endmacro -%}

{% macro fabricspark__options_clause() -%}
  {%- set options = config.get('options') -%}
  {%- if options is not none %}
    options (
      {%- for option in options -%}
      {{ option }} "{{ options[option] }}" {% if not loop.last %}, {% endif %}
      {%- endfor %}
    )
  {%- endif %}
{%- endmacro -%}

{% macro comment_clause() %}
  {{ return(adapter.dispatch('comment_clause', 'dbt')()) }}
{%- endmacro -%}

{% macro fabricspark__comment_clause() %}
  {%- set raw_persist_docs = config.get('persist_docs', {}) -%}

  {%- if raw_persist_docs is mapping -%}
    {%- set raw_relation = raw_persist_docs.get('relation', false) -%}
      {%- if raw_relation -%}
      comment '{{ model.description | replace("'", "\\'") }}'
      {% endif %}
  {%- elif raw_persist_docs -%}
    {{ exceptions.raise_compiler_error("Invalid value provided for 'persist_docs'. Expected dict but got value: " ~ raw_persist_docs) }}
  {% endif %}
{%- endmacro -%}

{% macro partition_cols(label, required=false) %}
  {{ return(adapter.dispatch('partition_cols', 'dbt')(label, required)) }}
{%- endmacro -%}

{% macro fabricspark__partition_cols(label, required=false) %}
  {%- set cols = config.get('partition_by', validator=validation.any[list, basestring]) -%}
  {%- if cols is not none %}
    {%- if cols is string -%}
      {%- set cols = [cols] -%}
    {%- endif -%}
    {{ label }} (
    {%- for item in cols -%}
      {{ item }}
      {%- if not loop.last -%},{%- endif -%}
    {%- endfor -%}
    )
  {%- endif %}
{%- endmacro -%}

{% macro clustered_cols(label, required=false) %}
  {{ return(adapter.dispatch('clustered_cols', 'dbt')(label, required)) }}
{%- endmacro -%}

{% macro fabricspark__clustered_cols(label, required=false) %}
  {%- set cols = config.get('clustered_by', validator=validation.any[list, basestring]) -%}
  {%- set buckets = config.get('buckets', validator=validation.any[int]) -%}
  {%- if (cols is not none) and (buckets is not none) %}
    {%- if cols is string -%}
      {%- set cols = [cols] -%}
    {%- endif -%}
    {{ label }} (
    {%- for item in cols -%}
      {{ item }}
      {%- if not loop.last -%},{%- endif -%}
    {%- endfor -%}
    ) into {{ buckets }} buckets
  {%- endif %}
{%- endmacro -%}

{% macro fetch_tbl_properties(relation) -%}
  {% call statement('list_properties', fetch_result=True) -%}
    SHOW TBLPROPERTIES {{ relation }}
  {% endcall %}
  {% do return(load_result('list_properties').table) %}
{%- endmacro %}

{% macro persist_constraints(relation, model) %}
  {{ return(adapter.dispatch('persist_constraints', 'dbt')(relation, model)) }}
{% endmacro %}

/* TODO: alter table {{ relation }} change column {{ quoted_name }} set not null;
     is not supported in Fabric Runtime 1.2/Spark 3.4.1
    {% do alter_column_set_constraints(relation, model.columns) %}
*/
{% macro fabricspark__persist_constraints(relation, model) %}
  {%- set contract_config = config.get('contract') -%}
  {% if contract_config.enforced and config.get('file_format', 'delta') == 'delta' %}
    {# {% do alter_column_set_constraints(relation, model.columns) %} #}
    {% do alter_table_add_constraints(relation, model.constraints) %}
  {% endif %}
{% endmacro %}

{% macro alter_table_add_constraints(relation, constraints) %}
  {{ return(adapter.dispatch('alter_table_add_constraints', 'dbt')(relation, constraints)) }}
{% endmacro %}

{% macro  fabricspark__alter_table_add_constraints(relation, constraints) %}
  {% for constraint in constraints %}
    {% if constraint.type == 'check' and not is_incremental() %}
      {%- set constraint_hash = local_md5(column_name ~ ";" ~ constraint.expression ~ ";" ~ loop.index) -%}
      {% call statement() %}
        alter table {{ relation }} add constraint {{ constraint.name if constraint.name else constraint_hash }} check ({{ constraint.expression }});
      {% endcall %}
    {% endif %}
  {% endfor %}
{% endmacro %}


{% macro alter_column_set_constraints(relation, column_dict) %}
  return(adapter.dispatch('alter_column_set_constraints', 'dbt')(relation, column_dict))
{% endmacro %}

{% macro fabricspark__alter_column_set_constraints(relation, column_dict) %}

  {% for column_name in column_dict %}
    {% set constraints = column_dict[column_name]['constraints'] %}
    {% for constraint in constraints %}
      {% if constraint.type != 'not_null' %}
        {{ exceptions.warn('Invalid constraint for column ' ~ column_name ~ '. Only `not_null` is supported.') }}
      {% else %}
        {% set quoted_name = adapter.quote(column_name) if column_dict[column_name]['quote'] else column_name %}
        {% call statement() %}
          alter table {{ relation }} change column {{ quoted_name }} set not null {{ constraint.expression or "" }};
        {% endcall %}
      {% endif %}
    {% endfor %}
  {% endfor %}
{% endmacro %}
