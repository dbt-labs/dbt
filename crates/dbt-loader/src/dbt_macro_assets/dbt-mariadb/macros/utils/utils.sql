{#- MariaDB treats `||` as logical OR (unless PIPES_AS_CONCAT is set) and
    CAST only accepts a few targets (CHAR, SIGNED, DECIMAL, DOUBLE, DATE,
    DATETIME, ...; no TEXT, BIGINT or BOOLEAN), so the Postgres-flavored
    defaults need MariaDB spellings. https://mariadb.com/kb/en/cast/ -#}

{% macro mariadb__concat(fields) -%}
    concat({{ fields|join(', ') }})
{%- endmacro %}

{% macro mariadb__hash(field) -%}
    md5(cast({{ field }} as char))
{%- endmacro %}

{% macro mariadb__snapshot_hash_arguments(args) -%}
    md5(concat_ws('|', {%- for arg in args -%}
        coalesce(cast({{ arg }} as char), '')
        {%- if not loop.last %}, {% endif -%}
    {%- endfor -%}))
{%- endmacro %}

{% macro mariadb__cast_bool_to_text(field) -%}
    case when {{ field }} then 'true' when not {{ field }} then 'false' end
{%- endmacro %}

{#- The type macros are used as CAST targets. -#}
{% macro mariadb__type_string() -%} char {%- endmacro %}
{% macro mariadb__type_timestamp() -%} datetime(6) {%- endmacro %}
{% macro mariadb__type_float() -%} double {%- endmacro %}
{% macro mariadb__type_numeric() -%} decimal(28,6) {%- endmacro %}
{% macro mariadb__type_bigint() -%} signed {%- endmacro %}
{% macro mariadb__type_int() -%} signed {%- endmacro %}
{% macro mariadb__type_boolean() -%} signed {%- endmacro %}

{#- Maps a column type (as `api.Column.translate_type` returns it) to a CAST
    target. -#}
{% macro mariadb__cast_target(type) -%}
    {%- set lowered = type | trim | lower -%}
    {%- set base = lowered.split('(')[0].split(' ')[0] -%}
    {%- if base in ['tinyint', 'smallint', 'mediumint', 'int', 'integer', 'bigint', 'bool', 'boolean', 'bit', 'year'] -%}
        {{- 'unsigned' if 'unsigned' in lowered else 'signed' -}}
    {%- elif base in ['char', 'varchar', 'tinytext', 'text', 'mediumtext', 'longtext', 'enum', 'set', 'json', 'uuid', 'string'] -%}
        char
    {%- elif base in ['binary', 'varbinary', 'tinyblob', 'blob', 'mediumblob', 'longblob'] -%}
        binary
    {%- elif base in ['real', 'double', 'float8'] -%}
        double
    {%- elif base == 'timestamp' -%}
        datetime(6)
    {%- elif base in ['numeric', 'dec', 'fixed'] -%}
        decimal{{ lowered[base | length:] }}
    {%- else -%}
        {{ type }}
    {%- endif -%}
{%- endmacro %}

{% macro mariadb__cast(field, type) -%}
    cast({{ field }} as {{ mariadb__cast_target(type) }})
{%- endmacro %}

{#- MariaDB CAST turns a bad value into 0 (numbers) or fails the statement
    in strict mode, so values are checked first. A date with an impossible
    day (2024-02-30) still fails in strict mode. -#}
{% macro mariadb__safe_cast(field, type) -%}
    {%- set target = mariadb__cast_target(type) | trim -%}
    {%- set base = target | lower | replace('(', ' ') | trim -%}
    {%- set base = base.split(' ')[0] -%}
    {%- if base in ['signed', 'unsigned'] -%}
        case when trim({{ field }}) regexp '^[+-]?[0-9]+$' then cast({{ field }} as {{ target }}) end
    {%- elif base in ['decimal', 'double', 'float'] -%}
        case when trim({{ field }}) regexp '^[+-]?([0-9]+[.]?[0-9]*|[.][0-9]+)([eE][+-]?[0-9]+)?$' then cast({{ field }} as {{ target }}) end
    {%- elif base in ['date', 'datetime'] -%}
        case when trim({{ field }}) regexp '^[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])([ T]([01][0-9]|2[0-3]):[0-5][0-9](:[0-5][0-9]([.][0-9]{1,6})?)?)?$' then cast({{ field }} as {{ target }}) end
    {%- else -%}
        cast({{ field }} as {{ target }})
    {%- endif -%}
{%- endmacro %}

{% macro mariadb__dateadd(datepart, interval, from_date_or_timestamp) -%}
    date_add({{ from_date_or_timestamp }}, interval {{ interval }} {{ datepart }})
{%- endmacro %}

{#- dbt counts the datepart boundaries crossed between the two values
    (2024-01-31 to 2024-02-01 is one month), while TIMESTAMPDIFF counts whole
    elapsed units, so both values are truncated first. -#}
{% macro mariadb__datediff(first_date, second_date, datepart) -%}
    {%- if datepart | lower in ['millisecond', 'microsecond'] -%}
        timestampdiff(microsecond, {{ first_date }}, {{ second_date }}){{ ' div 1000' if datepart | lower == 'millisecond' }}
    {%- else -%}
        timestampdiff({{ datepart }}, {{ mariadb__date_trunc(datepart, first_date) }}, {{ mariadb__date_trunc(datepart, second_date) }})
    {%- endif -%}
{%- endmacro %}

{% macro mariadb__date_trunc(datepart, date) -%}
    {%- set datepart = datepart | lower -%}
    {%- if datepart == 'year' -%}
        cast(date_format({{ date }}, '%Y-01-01') as datetime)
    {%- elif datepart == 'quarter' -%}
        cast(makedate(year({{ date }}), 1) + interval (quarter({{ date }}) - 1) quarter as datetime)
    {%- elif datepart == 'month' -%}
        cast(date_format({{ date }}, '%Y-%m-01') as datetime)
    {%- elif datepart == 'week' -%}
        cast(date({{ date }}) - interval weekday({{ date }}) day as datetime)
    {%- elif datepart == 'day' -%}
        cast(date({{ date }}) as datetime)
    {%- elif datepart == 'hour' -%}
        cast(date_format({{ date }}, '%Y-%m-%d %H:00:00') as datetime)
    {%- elif datepart == 'minute' -%}
        cast(date_format({{ date }}, '%Y-%m-%d %H:%i:00') as datetime)
    {%- elif datepart == 'second' -%}
        cast(date_format({{ date }}, '%Y-%m-%d %H:%i:%s') as datetime)
    {%- else -%}
        {{ exceptions.raise_compiler_error("Unsupported datepart for date_trunc on MariaDB: " ~ datepart) }}
    {%- endif -%}
{%- endmacro %}

{% macro mariadb__last_day(date, datepart) -%}
    {%- if datepart | lower == 'month' -%}
        last_day({{ date }})
    {%- else -%}
        {{ dbt.default_last_day(date, datepart) }}
    {%- endif -%}
{%- endmacro %}

{#- Out-of-range parts return '', as split_part does on Postgres. -#}
{% macro mariadb__split_part(string_text, delimiter_text, part_number) -%}
    {%- set part_count -%}
        ((char_length({{ string_text }}) - char_length(replace({{ string_text }}, {{ delimiter_text }}, ''))) div char_length({{ delimiter_text }}) + 1)
    {%- endset -%}
    case when {{ part_number | abs }} > {{ part_count }} then '' else
    {%- if part_number >= 0 %}
        substring_index(substring_index({{ string_text }}, {{ delimiter_text }}, {{ part_number }}), {{ delimiter_text }}, -1)
    {%- else %}
        substring_index(substring_index({{ string_text }}, {{ delimiter_text }}, {{ part_number }}), {{ delimiter_text }}, 1)
    {%- endif %}
    end
{%- endmacro %}

{#- GROUP_CONCAT cuts its result at group_concat_max_len (1 MB by default)
    with only a warning; raise it in the session for longer lists. -#}
{% macro mariadb__listagg(measure, delimiter_text, order_by_clause, limit_num) -%}
    group_concat(
        {{ measure }}
        {% if order_by_clause -%} {{ order_by_clause }} {%- endif %}
        separator {{ delimiter_text }}
        {% if limit_num -%} limit {{ limit_num }} {%- endif %}
    )
{%- endmacro %}
