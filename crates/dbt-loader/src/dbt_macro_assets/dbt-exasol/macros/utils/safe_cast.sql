{% macro exasol__safe_cast(field, type) -%}
    {#- Exasol has no TRY_CAST and plain CAST throws on bad input. Guard with the
        IS_* validators (which accept string or typed args) so a bad value yields
        NULL, honoring safe_cast's non-throwing contract. String targets need no
        guard. -#}
    {%- set t = type | lower -%}
    {%- if t == "boolean" -%}
        case when is_boolean({{ field }}) then cast({{ field }} as {{ type }}) else null end
    {%- elif t.startswith("timestamp") -%}
        case when is_timestamp({{ field }}) then cast({{ field }} as {{ type }}) else null end
    {%- elif t == "date" -%}
        case when is_date({{ field }}) then cast({{ field }} as {{ type }}) else null end
    {%- elif t.startswith("decimal") or t.startswith("numeric") or t.startswith("number") or t.startswith("double") or t in ["int", "integer", "bigint", "smallint", "tinyint", "float", "real"] -%}
        case when is_number({{ field }}) then cast({{ field }} as {{ type }}) else null end
    {%- else -%}
        cast({{ field }} as {{ type }})
    {%- endif -%}
{%- endmacro %}
