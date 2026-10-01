{% macro spark__dateadd(datepart, interval, from_date_or_timestamp) %}

    {%- set clock_component -%}
        {# make sure the dates + timestamps are real, otherwise raise an error asap #}
        to_unix_timestamp({{ assert_not_null('to_timestamp', from_date_or_timestamp) }})
        - to_unix_timestamp({{ assert_not_null('date', from_date_or_timestamp) }})
    {%- endset -%}

    {%- if datepart in ['day', 'week'] -%}

        {%- set multiplier = 7 if datepart == 'week' else 1 -%}

        to_timestamp(
            to_unix_timestamp(
                date_add(
                    {{ assert_not_null('date', from_date_or_timestamp) }},
                    cast({{interval}} * {{multiplier}} as int)
                )
            ) + {{clock_component}}
        )

    {%- elif datepart in ['month', 'quarter', 'year'] -%}

        {%- set multiplier -%}
            {%- if datepart == 'month' -%} 1
            {%- elif datepart == 'quarter' -%} 3
            {%- elif datepart == 'year' -%} 12
            {%- endif -%}
        {%- endset -%}

        to_timestamp(
            to_unix_timestamp(
                add_months(
                    {{ assert_not_null('date', from_date_or_timestamp) }},
                    cast({{interval}} * {{multiplier}} as int)
                )
            ) + {{clock_component}}
        )

    {%- elif datepart in ('hour', 'minute', 'second', 'millisecond', 'microsecond') -%}

        {%- set multiplier -%}
            {%- if datepart == 'hour' -%} 3600
            {%- elif datepart == 'minute' -%} 60
            {%- elif datepart == 'second' -%} 1
            {%- elif datepart == 'millisecond' -%} (1/1000000)
            {%- elif datepart == 'microsecond' -%} (1/1000000)
            {%- endif -%}
        {%- endset -%}

        to_timestamp(
            {{ assert_not_null('to_unix_timestamp', from_date_or_timestamp) }}
            + cast({{interval}} * {{multiplier}} as int)
        )

    {%- else -%}

        {{ exceptions.raise_compiler_error("macro dateadd not implemented for datepart ~ '" ~ datepart ~ "' ~ on Spark") }}

    {%- endif -%}

{% endmacro %}
