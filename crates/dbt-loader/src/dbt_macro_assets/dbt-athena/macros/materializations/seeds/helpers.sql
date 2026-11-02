{% macro default__reset_csv_table(model, full_refresh, old_relation, agate_table) %}
    {% set sql = "" %}
    -- No truncate in Athena so always drop CSV table and recreate
    {{ drop_relation(old_relation) }}
    {% set sql = create_csv_table(model, agate_table) %}

    {{ return(sql) }}
{% endmacro %}

{% macro try_cast_timestamp(col) %}
    {% set date_formats = [
      '%Y-%m-%d %H:%i:%s',
      '%Y/%m/%d %H:%i:%s',
      '%d %M %Y %H:%i:%s',
      '%d/%m/%Y %H:%i:%s',
      '%d-%m-%Y %H:%i:%s',
      '%Y-%m-%d %H:%i:%s.%f',
      '%Y/%m/%d %H:%i:%s.%f',
      '%d %M %Y %H:%i:%s.%f',
      '%d/%m/%Y %H:%i:%s.%f',
      '%Y-%m-%dT%H:%i:%s.%fZ',
      '%Y-%m-%dT%H:%i:%sZ',
      '%Y-%m-%dT%H:%i:%s',
    ]%}

    coalesce(
      {% for date_format in date_formats %}
        try(date_parse({{ col }}, '{{ date_format }}'))
        {%- if not loop.last -%}, {% endif -%}
      {% endfor %}
    ) as {{ col }}
{% endmacro %}

{% macro create_csv_table_insert(model, agate_table) %}
  {%- set identifier = model['alias'] -%}

  {%- set lf_tags_config = config.get('lf_tags_config') -%}
  {%- set lf_grants = config.get('lf_grants') -%}
  {%- set column_override = config.get('column_types', {}) -%}
  {%- set quote_seed_column = config.get('quote_columns') -%}
  {%- set s3_data_dir = config.get('s3_data_dir', target.s3_data_dir) -%}
  {%- set s3_data_naming = config.get('s3_data_naming', target.s3_data_naming) -%}
  {%- set s3_tmp_table_dir = config.get('s3_tmp_table_dir', default=target.s3_tmp_table_dir) -%}
  {%- set external_location = config.get('external_location') -%}

  {%- set relation = api.Relation.create(
    identifier=identifier,
    schema=model.schema,
    database=model.database,
    type='table'
  ) -%}

  {%- set location = adapter.generate_s3_location(relation,
                                                 s3_data_dir,
                                                 s3_data_naming,
                                                 s3_tmp_table_dir,
                                                 external_location,
                                                 temporary) -%}

  {% set sql_table %}
    create external table {{ relation.render_hive() }} (
        {%- for col_name in agate_table.column_names -%}
          {%- set inferred_type = adapter.convert_type(agate_table, loop.index0) -%}
          {%- set type = column_override.get(col_name, inferred_type) -%}
          {%- set type = type if type != "string" else "varchar" -%}
          {%- set column_name = (col_name | string) -%}
          {{ adapter.quote_seed_column(column_name, quote_seed_column, "`") }} {{ ddl_data_type(type) }} {%- if not loop.last -%}, {% endif -%}
        {%- endfor -%}
    )
    location '{{ location }}'

  {% endset %}

  {% call statement('_') -%}
    {{ sql_table }}
  {%- endcall %}

  {% if lf_tags_config is not none %}
    {{ adapter.add_lf_tags(relation, lf_tags_config) }}
  {% endif %}

  {% if lf_grants is not none %}
    {{ adapter.apply_lf_grants(relation, lf_grants) }}
  {% endif %}

  {{ return(sql) }}
{% endmacro %}


{% macro create_csv_table_upload(model, agate_table) %}
  {%- set identifier = model['alias'] -%}

  {%- set lf_tags_config = config.get('lf_tags_config') -%}
  {%- set lf_grants = config.get('lf_grants') -%}

  {%- set column_override = config.get('column_types', {}) -%}
  {%- set quote_seed_column = config.get('quote_columns', None) -%}
  {%- set s3_data_dir = config.get('s3_data_dir', default=target.s3_data_dir) -%}
  {%- set s3_data_naming = config.get('s3_data_naming', target.s3_data_naming) -%}
  {%- set external_location = config.get('external_location', default=none) -%}
  {%- set seed_s3_upload_args = config.get('seed_s3_upload_args', default=target.seed_s3_upload_args) -%}

  {%- set tmp_relation = api.Relation.create(
    identifier=identifier + "__dbt_tmp",
    schema=model.schema,
    database=model.database,
    type='table'
  ) -%}

  {%- set tmp_s3_location = adapter.upload_seed_to_s3(
    tmp_relation,
    agate_table,
    s3_data_dir,
    s3_data_naming,
    external_location,
    seed_s3_upload_args=seed_s3_upload_args
  ) -%}

  -- create target relation
  {%- set relation = api.Relation.create(
    identifier=identifier,
    schema=model.schema,
    database=model.database,
    type='table'
  ) -%}

  -- drop tmp relation if exists
  {{ drop_relation(tmp_relation) }}

  {% set sql_tmp_table %}
    create external table {{ tmp_relation.render_hive() }} (
        {%- for col_name in agate_table.column_names -%}
            {%- set column_name = (col_name | string) -%}
            {{ adapter.quote_seed_column(column_name, quote_seed_column, "`") }} string {%- if not loop.last -%}, {% endif -%}
        {%- endfor -%}
    )
    row format serde 'org.apache.hadoop.hive.serde2.OpenCSVSerde'
    location '{{ tmp_s3_location }}'
    tblproperties (
      'skip.header.line.count'='1'
    )
  {% endset %}

  -- casting to type string is not allowed needs to be varchar
  {% set sql %}
    select
        {% for col_name in agate_table.column_names -%}
            {%- set inferred_type = adapter.convert_type(agate_table, loop.index0) -%}
            {%- set type = column_override.get(col_name, inferred_type) -%}
            {%- set type = type if type != "string" else "varchar" -%}
            {%- set column_name = (col_name | string) -%}
            {%- set quoted_column_name = adapter.quote_seed_column(column_name, quote_seed_column) -%}
            {% if type == 'timestamp' %}
              {{ try_cast_timestamp(quoted_column_name) }}
            {% else %}
              cast(nullif({{quoted_column_name}}, '') as {{ type }}) as {{quoted_column_name}}
            {% endif %}
            {%- if not loop.last -%}, {% endif -%}
        {%- endfor %}
    from
        {{ tmp_relation }}
  {% endset %}

  -- create tmp table
  {% call statement('_') -%}
    {{ sql_tmp_table }}
  {%- endcall -%}

  -- create target table from tmp table
  {% set sql_table = create_table_as(false, relation, sql)  %}
  {% call statement('_') -%}
    {{ sql_table }}
  {%- endcall %}

  -- drop tmp table
  {{ drop_relation(tmp_relation) }}

  -- delete csv file from s3
  {% do adapter.delete_from_s3(tmp_s3_location) %}

  {% if lf_tags_config is not none %}
    {{ adapter.add_lf_tags(relation, lf_tags_config) }}
  {% endif %}

  {% if lf_grants is not none %}
    {{ adapter.apply_lf_grants(relation, lf_grants) }}
  {% endif %}

  {{ return(sql_table) }}
{% endmacro %}

{% macro athena__create_csv_table(model, agate_table) %}

  {%- set seed_by_insert = config.get('seed_by_insert', False) | as_bool -%}

  {%- if seed_by_insert -%}
    {% do log('seed by insert...') %}
    {%- set sql_table = create_csv_table_insert(model, agate_table) -%}
  {%- else -%}
    {% do log('seed by upload...') %}
    {%- set sql_table = create_csv_table_upload(model, agate_table) -%}
  {%- endif -%}

  {{ return(sql_table) }}
{% endmacro %}

{# Overwrite to satisfy dbt-core logic #}
{% macro athena__load_csv_rows(model, agate_table) %}
    {%- set seed_by_insert = config.get('seed_by_insert', False) | as_bool -%}
  {%- if seed_by_insert %}
    {{ default__load_csv_rows(model, agate_table) }}
  {%- else -%}
    select 1
  {% endif %}
{% endmacro %}
