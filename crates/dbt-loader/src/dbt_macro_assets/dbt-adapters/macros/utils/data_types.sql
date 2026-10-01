{# string  -------------------------------------------------     #}

-- funcsign: () -> string
{%- macro type_string() -%}
  {{ return(adapter.dispatch('type_string', 'dbt')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro default__type_string() %}
    {{ return(api.Column.translate_type("string")) }}
{% endmacro %}

-- This will return 'text' by default
-- On Postgres + Snowflake, that's equivalent to varchar (no size)
-- Redshift will treat that as varchar(256)


{# timestamp  -------------------------------------------------     #}

-- funcsign: () -> string
{%- macro type_timestamp() -%}
  {{ return(adapter.dispatch('type_timestamp', 'dbt')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro default__type_timestamp() %}
    {{ return(api.Column.translate_type("timestamp")) }}
{% endmacro %}

/*
POSTGRES
https://www.postgresql.org/docs/current/datatype-datetime.html:
The SQL standard requires that writing just `timestamp`
be equivalent to `timestamp without time zone`, and
PostgreSQL honors that behavior.
`timestamptz` is accepted as an abbreviation for `timestamp with time zone`;
this is a PostgreSQL extension.

SNOWFLAKE
https://docs.snowflake.com/en/sql-reference/data-types-datetime.html#timestamp
The TIMESTAMP_* variation associated with TIMESTAMP is specified by the
TIMESTAMP_TYPE_MAPPING session parameter. The default is TIMESTAMP_NTZ.

BIGQUERY
TIMESTAMP means 'timestamp with time zone'
DATETIME means 'timestamp without time zone'
TODO: shouldn't this return DATETIME instead of TIMESTAMP, for consistency with other databases?
e.g. dateadd returns a DATETIME

/* Snowflake:
https://docs.snowflake.com/en/sql-reference/data-types-datetime.html#timestamp
The TIMESTAMP_* variation associated with TIMESTAMP is specified by the TIMESTAMP_TYPE_MAPPING session parameter. The default is TIMESTAMP_NTZ.
*/


{# float  -------------------------------------------------     #}

{%- macro type_float() -%}
  {{ return(adapter.dispatch('type_float', 'dbt')()) }}
{%- endmacro -%}

{% macro default__type_float() %}
    {{ return(api.Column.translate_type("float")) }}
{% endmacro %}

{# numeric  -------------------------------------------------     #}

{%- macro type_numeric() -%}
  {{ return(adapter.dispatch('type_numeric', 'dbt')()) }}
{%- endmacro -%}

/*
This one can't be just translate_type, since precision/scale make it a bit more complicated.

On most databases, the default (precision, scale) is something like:
  Redshift: (18, 0)
  Snowflake: (38, 0)
  Postgres: (<=131072, 0)

https://www.postgresql.org/docs/current/datatype-numeric.html:
Specifying NUMERIC without any precision or scale creates an “unconstrained numeric”
column in which numeric values of any length can be stored, up to the implementation limits.
A column of this kind will not coerce input values to any particular scale,
whereas numeric columns with a declared scale will coerce input values to that scale.
(The SQL standard requires a default scale of 0, i.e., coercion to integer precision.
We find this a bit useless. If you're concerned about portability, always specify
the precision and scale explicitly.)
*/

{% macro default__type_numeric() %}
    {{ return(api.Column.numeric_type("numeric", 28, 6)) }} -- noqa: this warning is in the comment
{% endmacro %}


{# bigint  -------------------------------------------------     #}

{%- macro type_bigint() -%}
  {{ return(adapter.dispatch('type_bigint', 'dbt')()) }}
{%- endmacro -%}

-- We don't have a conversion type for 'bigint' in TYPE_LABELS,
-- so this actually just returns the string 'bigint'

{% macro default__type_bigint() %}
    {{ return(api.Column.translate_type("bigint")) }}
{% endmacro %}

-- Good news: BigQuery now supports 'bigint' (and 'int') as an alias for 'int64'

{# int  -------------------------------------------------     #}

{%- macro type_int() -%}
  {{ return(adapter.dispatch('type_int', 'dbt')()) }}
{%- endmacro -%}

{%- macro default__type_int() -%}
  {{ return(api.Column.translate_type("integer")) }}
{%- endmacro -%}

-- returns 'int' everywhere, except BigQuery, where it returns 'int64'
-- (but BigQuery also now accepts 'int' as a valid alias for 'int64')

{# bool  -------------------------------------------------     #}

{%- macro type_boolean() -%}
  {{ return(adapter.dispatch('type_boolean', 'dbt')()) }}
{%- endmacro -%}

{%- macro default__type_boolean() -%}
  {{ return(api.Column.translate_type("boolean")) }}
{%- endmacro -%}

-- returns 'boolean' everywhere. BigQuery accepts 'boolean' as a valid alias for 'bool'
