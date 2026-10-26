{% set table_name = 'c' %}
{% set db_name = target.database %}
{% set db_schema = target.schema %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% set relation = api.Relation.create(db_name, db_schema, table_name) %}
{% set columns = adapter.get_columns_in_relation(relation) %}

{% set num_col = columns[0] %}
{% set text_col = columns[1] %}
{% set text_col2 = columns[2] %}

{% set result = text_col.can_expand_to(other_column=num_col) %}
{{ print('text_col.can_expand_to(num_col): ' ~ result) }}
{% set result = text_col.can_expand_to(text_col2) %}
{{ print('text_col.can_expand_to(text_col2): ' ~ result) }}

{% for col in columns %}
    {# TODO: see why results from databricks have columns that we dont want to log #}
    {% if col.name | lower == 'number_col' or col.name | lower == 'text_col' %}
        {% do log("Column: " ~ col.name, info=True) %}
        {% do log("Column data_type: " ~ col.data_type, info=True) %}
        {% do log("Column dtype: " ~ col.dtype, info=True) %}
        {% do log("Column numeric_precision: " ~ col.numeric_precision, info=True) %}
        {% do log("Column numeric_scale: " ~ col.numeric_scale, info=True) %}
        {% do log("Column char_size: " ~ col.char_size, info=True) %}
        {% do log("Column quoted: " ~ col.quoted, info=True) %}
        {% if col.name | lower == 'text_col' %}
            {% do log("Column string_size: " ~ col.string_size(), info=True) %} -- databricks is expected to error on this, consistent with dbt 1.10.0-b2 (31cb5a9b724dd2d5f31542b5547fd6bfeb2e0afb)
        {% endif %}
    {% endif %}
{% endfor %}

select 1 as one
