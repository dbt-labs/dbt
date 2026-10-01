{%- macro get_create_sql_comment(comment) -%}
{% if comment is string -%}
  {#-- escape backslashes first so they cannot merge with the apostrophe escape below --#}
  COMMENT '{{ comment | replace("\\", "\\\\") | replace("'", "\\'") }}'
{%- endif -%}
{%- endmacro -%}

