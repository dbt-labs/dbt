-- Invalid identifier unless quoted
-- reference: https://docs.snowflake.com/en/sql-reference/identifiers-syntax
{{ config(
    database="123_I_NEED_QUOTES",
) }}

select 1 as id