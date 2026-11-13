{% macro create_weird_mask() %}
    CREATE FUNCTION IF NOT EXISTS {{ target.database }}.{{ t_schema_name() }}.weird_mask(
        password STRING,
        id STRING,
        literal STRING,
        num INT,
        bool_val BOOLEAN,
        null_val STRING,
        interval INTERVAL DAY
    )
    RETURNS STRING
    RETURN CONCAT(
        password, '-',
        id, '-',
        literal, '-',
        CAST(num AS STRING), '-',
        CAST(bool_val AS STRING), '-',
        COALESCE(null_val, 'NULL'), '-',
        CAST(interval AS STRING)
    );
{% endmacro %}

{% macro create_full_mask() %}
    CREATE FUNCTION IF NOT EXISTS {{ target.database }}.{{ t_schema_name() }}.full_mask(password STRING)
    RETURNS STRING
    RETURN '*****';
{% endmacro %}

{% macro create_email_mask() %}
    CREATE FUNCTION IF NOT EXISTS {{ target.database }}.{{ t_schema_name() }}.email_mask(value STRING)
    RETURNS STRING
    RETURN CONCAT(
        REPEAT('*', POSITION('@' IN value) - 1),
        SUBSTR(value, POSITION('@' IN value))
    );
{% endmacro %}

{% macro create_password_mask() %}
    CREATE FUNCTION IF NOT EXISTS {{ target.database }}.{{ t_schema_name() }}.password_mask(
        password STRING,
        name STRING,
        literal_string STRING
    )
    RETURNS STRING
    RETURN CONCAT(
        '[', literal_string, '] ',
        name, ': ',
        LEFT(password, 1),
        REPEAT('*', LENGTH(password) - 2),
        RIGHT(password, 1)
    );
{% endmacro %}