-- Manually update a materialized view to override its settings
{% macro update_materialized_view(view_name) %}
    {{ print("update_materialized_view(" ~ view_name ~ ")") }}
    {% set sql %}
        ALTER MATERIALIZED VIEW `{{database}}`.`{{schema}}`.`{{view_name}}`
        SET OPTIONS(
            enable_refresh=false,
            refresh_interval_minutes=1234.0
        )
    {% endset %}
    {% do run_query(sql) %}
{% endmacro %}


-- Pull the applied options of a materialized view (directly from the engine)
{% macro pull_materialized_view_options(view_name) %}
    {{ print("pull_materialized_view_options(" ~ view_name ~ ")" ) }}
    {% set sql %}
        SELECT 
            option_name, option_value 
        FROM 
            `{{database}}`.`{{schema}}`.INFORMATION_SCHEMA.TABLE_OPTIONS 
        WHERE
            table_name = '{{view_name}}'
            AND option_name IN ('enable_refresh', 'refresh_interval_minutes')
    {% endset %}
    {% set results = run_query(sql) %}
    {{ print(results) }}
{% endmacro %}
