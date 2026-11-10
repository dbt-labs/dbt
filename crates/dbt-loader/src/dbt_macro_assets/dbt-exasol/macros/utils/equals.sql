{% macro exasol__equals(expr1, expr2) -%}
    {#- NULL-safe equality. Exasol stores '' as NULL, so plain (a = b) returns
        NULL for empty/NULL operands and silently drops rows in tests, merges and
        snapshot change-detection. DECODE compares NULL as equal to NULL, so this
        is null-safe regardless of the enable_truthy_nulls_equals_macro flag
        (which defaults off). -#}
    decode({{ expr1 }}, {{ expr2 }}, 1, 0) = 1
{%- endmacro %}
