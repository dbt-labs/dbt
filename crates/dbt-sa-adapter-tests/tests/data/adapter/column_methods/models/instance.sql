{% set str_col = api.Column.create("str_col", api.Column.string_type(), 10, None, None) %}
{% set str_col2 = api.Column.create("str_col2", api.Column.string_type(), 20, None, None) %}
{{ print(str_col) }}
{{ print("string_size   " ~ str_col.string_size()) }}
{{ print("is_string     " ~ str_col.is_string()) }}
{{ print("is_numeric    " ~ str_col.is_numeric()) }}
{{ print("is_float      " ~ str_col.is_float()) }}
{{ print("is_number     " ~ str_col.is_number()) }}
{{ print("is_integer    " ~ str_col.is_integer()) }}
{{ print("can_expand_to " ~ str_col.can_expand_to(str_col2)) }}

{% set int_col = api.Column.create("int_col", api.Column.numeric_type("NUMBER"), None, 10, 10) %}
{{ print(int_col) }}
{{ print("is_string     " ~ int_col.is_string()) }}
{{ print("is_numeric    " ~ int_col.is_numeric()) }}
{{ print("is_float      " ~ int_col.is_float()) }}
{{ print("is_number     " ~ int_col.is_number()) }}
{{ print("is_integer    " ~ int_col.is_integer()) }}

select 1 as id
