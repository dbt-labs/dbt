-- funcsign: (model, bool, relation, agate_table) -> string
{% macro snowflake__reset_csv_table(model, full_refresh, old_relation, agate_table) %}
    {#--
        On non-full-refresh seeds, skip the explicit TRUNCATE. INSERT OVERWRITE in
        snowflake__load_csv_rows will atomically clear and repopulate the table in a
        single DML statement, closing the window where concurrent readers see an empty table.
        (TRUNCATE is DDL in Snowflake and auto-commits, creating that window.)

        Exception: when --empty is used, load_csv_rows is skipped entirely, so we
        must truncate here to clear the table.
    --#}
    {% set sql = "" %}
    {% if full_refresh %}
        {{ adapter.drop_relation(old_relation) }}
        {% set sql = create_csv_table(model, agate_table) %}
    {% elif (agate_table.rows | length) == 0 %}
        {{ adapter.truncate_relation(old_relation) }}
    {% endif %}
    {{ return(sql) }}
{% endmacro %}

-- funcsign: (model, agate_table) -> string
{% macro snowflake__load_csv_rows(model, agate_table) %}
    {#--
        Wrap all INSERT batches in a single transaction. The first batch uses
        INSERT OVERWRITE, which atomically clears the table and inserts new rows as
        one DML operation. Subsequent batches append with regular INSERT. Everything
        commits together, so concurrent readers never see an empty or partial table.

        For catalog-linked databases (e.g. Glue-backed Iceberg), explicit transaction
        wrapping is skipped to stay consistent with the rest of the adapter.
    --#}
    {% set batch_size = get_batch_size() %}
    {% set cols_sql = get_seed_column_quoted_csv(model, agate_table.column_names) %}
    {% set is_catalog_linked = snowflake__is_catalog_linked_database(relation=config.model) %}

    {#-- Handle empty seeds (header-only CSV) with an early return. The batching
         loop below would never run, so we issue a single INSERT OVERWRITE that
         selects zero rows to atomically clear the table. --#}
    {% if (agate_table.rows | length) == 0 %}
        {% set sql %}
            insert overwrite into {{ this.render() }} ({{ cols_sql }})
            select {{ cols_sql }} from {{ this.render() }} where 1=0
        {% endset %}
        {% if not is_catalog_linked %}
            {% do adapter.add_query('BEGIN', auto_begin=False) %}
        {% endif %}
        {% do adapter.add_query(sql, abridge_sql_log=True) %}
        {% if not is_catalog_linked %}
            {% do adapter.add_query('COMMIT', auto_begin=False) %}
        {% endif %}
        {{ return(sql) }}
    {% endif %}

    {% set statements = [] %}

    {% if not is_catalog_linked %}
        {% do adapter.add_query('BEGIN', auto_begin=False) %}
    {% endif %}

    {% for chunk in agate_table.rows | batch(batch_size) %}
        {% set bindings = [] %}

        {% for row in chunk %}
            {% do bindings.extend(row) %}
        {% endfor %}

        {% set sql %}
            insert {% if loop.first %}overwrite {% endif %}into {{ this.render() }} ({{ cols_sql }}) values
            {% for row in chunk -%}
                ({%- for column in agate_table.column_names -%}
                    %s
                    {%- if not loop.last%},{%- endif %}
                {%- endfor -%})
                {%- if not loop.last%},{%- endif %}
            {%- endfor %}
        {% endset %}

        {% do adapter.add_query(sql, bindings=bindings, abridge_sql_log=True) %}

        {% if loop.index0 == 0 %}
            {% do statements.append(sql) %}
        {% endif %}
    {% endfor %}

    {% if not is_catalog_linked %}
        {% do adapter.add_query('COMMIT', auto_begin=False) %}
    {% endif %}

    {# Return SQL so we can render it out into the compiled files #}
    {{ return(statements[0]) }}
{% endmacro %}

{% materialization seed, adapter='snowflake' %}
    {% set original_query_tag = set_query_tag() %}

    {% set relations = materialization_seed_default() %}

    {% do unset_query_tag(original_query_tag) %}

    {{ return(relations) }}
{% endmaterialization %}
