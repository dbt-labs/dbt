{% macro table_source(table_name) -%}
    {% set schema_results = run_query("show databases") %}
    {% set lh_names = [] %}
    {% for row in schema_results %}
        {{ log("Lakehouse found: " ~ row[0], info) }}
        {% do lh_names.append(row[0]) %}
    {% endfor %}
    
    {% for lh_name in lh_names %}
        {% set relation = adapter.get_relation(model.database, lh_name, table_name) %}
        
        {% if relation %}
            {{ log("Table " ~ table_name ~ " found in lakehouse " ~ lh_name, info) }}
            {{ return(lh_name ~ "." ~ table_name)}}
        {% endif %}
    {% endfor %}
    
    {{ log("Table " ~ table_name ~ " not found in any lakehouse", warning) }}
    {{ return(None) }}
{%- endmacro -%}