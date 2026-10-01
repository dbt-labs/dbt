{% macro clickhouse__get_test_sql(main_sql, fail_calc, warn_if, error_if, limit) -%}
    {% set main_sql_formatted = clickhouse__place_limit(main_sql, limit) if limit !=None else main_sql%}
    select
      {{ fail_calc }} as failures,
      {{ fail_calc }} {{ warn_if }} as should_warn,
      {{ fail_calc }} {{ error_if }} as should_error
    from (
      {{ main_sql_formatted }}
    ) dbt_internal_test

{%- endmacro %}


-- This macro is designed to add a LIMIT clause to a ClickHouse SQL query while preserving any ClickHouse settings specified in the query.
-- When multiple queries are nested, the limit will be attached to the outer query
{% macro clickhouse__place_limit(query, limit) -%}
   {% if 'settings' in query.lower()%}
        {% if '-- end_of_sql' not in query.lower()%}
            {{exceptions.raise_compiler_error("-- end_of_sql must be set when using ClickHouse settings")}}
        {% endif %}
        {% set split_by_settings_sections = query.split("-- end_of_sql")%}
        {% set split_by_settings_sections_with_limit = split_by_settings_sections[-2] + "\n LIMIT " + limit|string  + "\n" %}
        {% set query_with_limit = "-- end_of_sql".join(split_by_settings_sections[:-2] + [split_by_settings_sections_with_limit, split_by_settings_sections[-1]])%}
        {{query_with_limit}}
    {% else %}
    {{query}}
    {{"limit " ~ limit}}
    {% endif %}
{%- endmacro %}

{% macro clickhouse__any_value(expression) -%}
    any({{ expression }})
{%- endmacro %}


{% macro clickhouse__bool_or(expression) -%}
    max({{ expression }}) > 0
{%- endmacro %}


{% macro clickhouse__cast_bool_to_text(field) %}
    multiIf({{ field }} > 0, 'true', {{ field }} = 0, 'false', NULL)
{% endmacro %}


{% macro clickhouse__hash(field) -%}
    lower(hex(MD5(toString({{ field }} ))))
{%- endmacro %}


{%- macro clickhouse__last_day(date, datepart) -%}
    {{ dbt.dateadd('day', '-1', dbt.dateadd(datepart, '1', dbt.date_trunc(datepart, date)))}}
{%- endmacro -%}


{% macro clickhouse__split_part(string_text, delimiter_text, part_number) %}
    splitByString({{delimiter_text}}, {{ string_text }})[{{ part_number }}]
{% endmacro %}


{% macro clickhouse__replace(field, old_chars, new_chars) %}
   replaceAll({{ field }},'{{ old_chars }}','{{ new_chars }}')
{% endmacro %}


{% macro clickhouse__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
    {% if order_by_clause and 'order by' == ' '.join(order_by_clause.split()[:2]).lower() -%}
      {% set order_by_clause_tokens = order_by_clause.split() %}
      {% if ',' in order_by_clause_tokens %}
        {{ exceptions.raise_compiler_error(
          'ClickHouse does not support multiple order by fields.')
        }}
      {%- endif  %}
      {% set order_by_clause_tokens = order_by_clause_tokens[2:] %}
      {% set sort_direction = '' %}
      {% if 'desc' in ''.join(order_by_clause_tokens[1:]).lower() %}
        {% set sort_direction = 'Reverse' %}
      {% endif %}
      {% set order_by_field = order_by_clause_tokens[0] %}

      {% set arr = "arrayMap(x -> x.1, array{}Sort(x -> x.2, arrayZip(array_agg({}), array_agg({}))))".format(sort_direction, measure, order_by_field) %}
    {% else -%}
      {% set arr = "array_agg({})".format(measure) %}
    {%- endif %}

    {% if limit_num -%}
      arrayStringConcat(arraySlice({{ arr }}, 1, {{ limit_num }}), {{delimiter_text}})
    {% else -%}
      arrayStringConcat({{ arr }}, {{delimiter_text}})
    {%- endif %}
{%- endmacro %}


{% macro clickhouse__array_construct(inputs, data_type) -%}
    {% if inputs|length > 0 %}
    [ {{ inputs|join(' , ') }} ]
    {% else %}
    emptyArray{{data_type}}()
    {% endif %}
{%- endmacro %}


{% macro clickhouse__array_append(array, new_element) -%}
    arrayPushBack({{ array }}, {{ new_element }})
{% endmacro %}


{% macro clickhouse__array_concat(array_1, array_2) -%}
   arrayConcat({{ array_1 }}, {{ array_2 }})
{% endmacro %}
