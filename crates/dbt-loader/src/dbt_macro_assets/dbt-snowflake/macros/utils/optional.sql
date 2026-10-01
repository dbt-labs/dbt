-- funcsign: (string, ANY, optional[string], optional[string]) -> string
{% macro optional(name, value, quote_char = '', equals_char = '= ') %}
{#-
--  Insert optional DDL parameters only when their value is provided; makes DDL statements more readable
--
--  Args:
--  - name: the name of the DDL option
--  - value: the value of the DDL option, may be None
--  - quote_char: the quote character to use (e.g. '"', '(', etc.), leave blank if unnecessary
--  - equals_char: the equals character to use (e.g. '= ')
--  Returns:
--      If the value is not None (e.g. provided by the user), return the option setting DDL
--      If the value is None, return an empty string
-#}
{%- set quote_char_right = ')' if quote_char == '(' else quote_char -%}
{% if value is not none %}{{ name }} {{ equals_char }}{{ quote_char }}{{ value }}{{ quote_char_right }}{% endif %}
{% endmacro %}
