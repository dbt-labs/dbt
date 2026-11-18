{% set flags = adapter.behavior %}
{{ print(flags) }}

{% if target.type == 'snowflake' %}
  {% set f = adapter.behavior.snowflake_default_transient_dynamic_tables %}
  {{ print("snowflake_default_transient_dynamic_tables.no_warn:" ~ f.no_warn) }}
{% elif target.type == 'databricks' %}
  {% set f = adapter.behavior.use_user_folder_for_python %}
  {{ print("use_user_folder_for_python.no_warn:" ~ f.no_warn) }}
  {% set f = adapter.behavior.use_materialization_v2 %}
  {{ print("use_materialization_v2.no_warn:" ~ f.no_warn) }}
{% endif %}

{% set f = flags.is_true %}
{{ print(f) }}

{% if f %}
  {{ print("expected") }}
{% else %}
  {{ print("not expected") }}
{% endif %}

{% set f = flags.is_false %}
{{ print(f) }}

{% if f %}
  {{ print("not expected") }}
{% else %}
  {{ print("expected") }}
{% endif %}

{% set f = flags.is_unknown %}
{{ print(f) }}

{% if f %}
  {{ print("not expected") }}
{% else %}
  {{ print("expected") }}
{% endif %}

SELECT 1
