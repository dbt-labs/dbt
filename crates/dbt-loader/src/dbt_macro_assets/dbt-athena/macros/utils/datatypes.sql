{# Implements Athena-specific datatypes where they differ from the dbt-core defaults #}
{# See https://docs.aws.amazon.com/athena/latest/ug/data-types.html #}

{%- macro athena__type_float() -%}
    DOUBLE
{%- endmacro -%}

{%- macro athena__type_numeric() -%}
    DECIMAL(38,6)
{%- endmacro -%}

{%- macro athena__type_int() -%}
    INTEGER
{%- endmacro -%}

{%- macro athena__type_string() -%}
    VARCHAR
{%- endmacro -%}
