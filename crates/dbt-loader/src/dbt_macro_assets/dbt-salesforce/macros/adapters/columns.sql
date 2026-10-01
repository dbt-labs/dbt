{# default get_empty_subquery_sql appears to be correct #}
{# default get_empty_schema_sql appears to be correct #}
{# default get_columns_in_query appears to be correct #}

{# These are almost certainly not possible since there is no way for proper DDL #}

-- funcsign: (relation, string, string) -> string
{% macro salesforce__alter_column_type(relation, column_name, new_column_type) -%}
{{ exceptions.raise_not_implemented('alter_column_type macro not implemented for adapter salesforce') }}
{% endmacro %}

-- funcsign: (relation, optional[list[base_column]], optional[list[base_column]]) -> string
{% macro salesforce__alter_relation_add_remove_columns(relation, add_columns, remove_columns) %}
{{ exceptions.raise_not_implemented('alter_relation_add_remove_columns macro not implemented for adapter salesforce') }}
{% endmacro %}
