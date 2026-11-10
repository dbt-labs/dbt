{# Exasol's string type is VARCHAR (up to the 2,000,000-char maximum). #}
-- funcsign: () -> string
{% macro exasol__type_string() %}
    varchar(2000000)
{% endmacro %}
