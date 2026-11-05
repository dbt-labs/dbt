{% macro trino__get_batch_size() %}
  {{ return(1000) }}
{% endmacro %}


{% macro create_bindings(row, types) %}
  {% set values = [] %}
  {% set re = modules.re %}

  {%- for item in row -%}
      {%- set type = types[loop.index0] -%}
      {%- set match_type = re.match("(\w+)(\(.*\))?", type) -%}
      {%- if item is not none and item is string and 'interval' in match_type.group(1) -%}
        {%- do values.append((none, match_type.group(1).upper() ~ " " ~ item)) -%}
      {%- elif item is not none and item is string and 'varchar' not in type.lower() -%}
        {%- do values.append((none, match_type.group(1).upper() ~ " '" ~ item ~ "'")) -%}
      {%- elif item is not none and 'varchar' in type.lower() -%}
        {%- do values.append((get_binding_char(), item|string())) -%}
      {%- else -%}
        {%- do values.append((get_binding_char(), item)) -%}
      {% endif -%}
  {%- endfor -%}
  {{ return(values) }}
{% endmacro %}


{#
  We need to override the default__load_csv_rows macro as Trino requires values to be typed according to the column type
  as in following example:

  create table "memory"."default"."string_type" ("varchar_example" varchar,"varchar_n_example" varchar(10),"char_example" char,"char_n_example" char(10),"varbinary_example" varbinary,"json_example" json)

  insert into "memory"."default"."string_type" ("varchar_example", "varchar_n_example", "char_example", "char_n_example", "varbinary_example", "json_example") values
          ('test','abc',CHAR 'd',CHAR 'ghi',VARBINARY '65683F',JSON '{"k1":1,"k2":23,"k3":456}'),(NULL,NULL,NULL,NULL,NULL,NULL)

  Usually seed row's values through agate_table's data type detection and come through as python types, in this case typing is
  handled by using bindings in `ConnectionWrapper.execute`. However dbt also allows you to override the data types of the created table
  through setting `column_types`, this case is handled here where we have the type information of the seed table.
#}

{% macro trino__load_csv_rows(model, agate_table) %}
  {% set column_override = model['config'].get('column_types', {}) %}
  {% set types = [] %}

  {%- for col_name in agate_table.column_names -%}
      {%- set inferred_type = adapter.convert_type(agate_table, loop.index0) -%}
      {%- set type = column_override.get(col_name, inferred_type) -%}
      {%- do types.append(type) -%}
  {%- endfor -%}

  {% set batch_size = get_batch_size() %}

  {% set cols_sql = get_seed_column_quoted_csv(model, agate_table.column_names) %}
  {% set bindings = [] %}

  {% set statements = [] %}

  {% for chunk in agate_table.rows | batch(batch_size) %}
      {% set bindings = [] %}

      {% set sql %}
          insert into {{ this.render() }} ({{ cols_sql }}) values
          {% for row in chunk -%}
              ({%- for tuple in create_bindings(row, types) -%}
                  {%- if tuple.0 is not none  -%}
                  {{ tuple.0 }}
                  {%- do bindings.append(tuple.1) -%}
                  {%- else -%}
                  {{ tuple.1 }}
                  {%- endif -%}
                  {%- if not loop.last%},{%- endif %}
              {%- endfor -%})
              {%- if not loop.last%},{%- endif %}
          {%- endfor %}
      {% endset %}

      {% do adapter.add_query(sql, bindings=bindings, abridge_sql_log=True) %}

      {% if loop.index0 == 0 %}
          {% do statements.append(sql) %}
      {% endif %}
  {% endfor %}

  {# Return SQL so we can render it out into the compiled files #}
  {{ return(statements[0]) }}
{% endmacro %}
