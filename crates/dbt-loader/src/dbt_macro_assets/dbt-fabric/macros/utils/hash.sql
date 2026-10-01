{% macro fabric__hash(field) %}
    lower(convert(varchar(50), hashbytes('md5', coalesce(convert(varchar(8000), {{field}}), '')), 2))
{% endmacro %}
