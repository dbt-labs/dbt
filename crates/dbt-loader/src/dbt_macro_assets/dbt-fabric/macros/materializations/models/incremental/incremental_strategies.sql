{% macro fabric__get_incremental_default_sql(arg_dict) %}

    {% if arg_dict["unique_key"] %}
        -- Delete + Insert Strategy, calls get_delete_insert_merge_sql
        {% do return(get_incremental_merge_sql(arg_dict)) %}
    {% else %}
        -- Incremental Append will insert data into target table.
        {% do return(get_incremental_append_sql(arg_dict)) %}
    {% endif %}

{% endmacro %}
