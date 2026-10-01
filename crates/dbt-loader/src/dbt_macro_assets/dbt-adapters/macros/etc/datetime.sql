-- funcsign: (string, string) -> timestamp
{% macro convert_datetime(date_str, date_fmt) %}

  {% set error_msg -%}
      The provided partition date '{{ date_str }}' does not match the expected format '{{ date_fmt }}'
  {%- endset %}

  {% set res = try_or_compiler_error(error_msg, modules.datetime.datetime.strptime, date_str.strip(), date_fmt) %}
  {{ return(res) }}

{% endmacro %}


-- funcsign: (string, optional[string], optional[string], optional[string]) -> list[timestamp]
{% macro dates_in_range(start_date_str, end_date_str=none, in_fmt="%Y%m%d", out_fmt="%Y%m%d") %}
    {% set end_date_str = start_date_str if end_date_str is none else end_date_str %}

    {% set start_date = convert_datetime(start_date_str, in_fmt) %}
    {% set end_date = convert_datetime(end_date_str, in_fmt) %}

    {% set day_count = (end_date - start_date).days %}
    {% if day_count < 0 %}
        {% set msg -%}
            Partition start date is after the end date ({{ start_date }}, {{ end_date }})
        {%- endset %}

        {{ exceptions.raise_compiler_error(msg, model) }}
    {% endif %}

    {% set date_list = [] %}
    {% for i in range(0, day_count + 1) %}
        {% set the_date = (modules.datetime.timedelta(days=i) + start_date) %}
        {% if not out_fmt %}
            {% set _ = date_list.append(the_date) %}
        {% else %}
            {% set _ = date_list.append(the_date.strftime(out_fmt)) %}
        {% endif %}
    {% endfor %}

    {{ return(date_list) }}
{% endmacro %}


-- funcsign: (string, optional[string]) -> list[timestamp]
{% macro partition_range(raw_partition_date, date_fmt='%Y%m%d') %}
    {% set partition_range = (raw_partition_date | string).split(",") %}

    {% if (partition_range | length) == 1 %}
      {% set start_date = partition_range[0] %}
      {% set end_date = none %}
    {% elif (partition_range | length) == 2 %}
      {% set start_date = partition_range[0] %}
      {% set end_date = partition_range[1] %}
    {% else %}
      {{ exceptions.raise_compiler_error("Invalid partition time. Expected format: {Start Date}[,{End Date}]. Got: " ~ raw_partition_date) }}
    {% endif %}

    {{ return(dates_in_range(start_date, end_date, in_fmt=date_fmt)) }}
{% endmacro %}


-- funcsign: () -> string
{% macro py_current_timestring() %}
    {% set dt = modules.datetime.datetime.now() %}
    {% do return(dt.strftime("%Y%m%d%H%M%S%f")) %}
{% endmacro %}
