{% macro duckdb__dateadd(datepart, interval, from_date_or_timestamp) %}

    {#
      Support both literal and expression intervals (e.g., column references)
      by multiplying an INTERVAL by the value. This avoids DuckDB parser issues
      with "interval (<expr>) <unit>" and works across versions.

      Also map unsupported units:
      - quarter => 3 months
      - week    => 7 days (DuckDB supports WEEK as a literal, but keep it explicit)
    #}

    {%- set unit = datepart | lower -%}
    {%- if unit == 'quarter' -%}
        ({{ from_date_or_timestamp }} + (cast({{ interval }} as bigint) * 3) * interval 1 month)
    {%- elif unit == 'week' -%}
        ({{ from_date_or_timestamp }} + (cast({{ interval }} as bigint) * 7) * interval 1 day)
    {%- else -%}
        ({{ from_date_or_timestamp }} + cast({{ interval }} as bigint) * interval 1 {{ unit }})
    {%- endif -%}

{% endmacro %}
