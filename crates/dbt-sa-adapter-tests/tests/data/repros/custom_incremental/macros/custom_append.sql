{% macro get_incremental_custom_append_sql(arg_dict) %}

{{ print("ENTERING get_incremental_custom_append_sql") }}
{{ return(get_incremental_append_sql(arg_dict)) }}

{% endmacro %}
