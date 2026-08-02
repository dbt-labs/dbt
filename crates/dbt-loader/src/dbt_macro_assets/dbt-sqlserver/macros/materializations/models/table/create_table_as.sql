{% macro sqlserver__create_clustered_columnstore_index(relation) -%}
    {#- cci_name embeds the schema, so it must be quoted as an identifier
        (raw only in the string comparison below) -- issue #409 -#}
    {%- set cci_name = (relation.schema ~ '_' ~ relation.identifier ~ '_cci') | replace(".", "") | replace(" ", "") -%}
    {%- set relation_name = relation.include(database=False) -%}
    {{ get_use_database_sql(relation.database) }}
    if EXISTS (
        SELECT *
        FROM sys.indexes {{ information_schema_hints() }}
        WHERE name = '{{ escape_single_quotes(cci_name) }}'
        AND object_id=object_id('{{ escape_single_quotes(relation_name) }}')
    )
    DROP index {{ relation_name }}.{{ adapter.quote(cci_name) }}
    CREATE CLUSTERED COLUMNSTORE INDEX {{ adapter.quote(cci_name) }}
    ON {{ relation_name }}
{% endmacro %}

{% macro sqlserver__create_table_as(temporary, relation, sql) -%}
    {%- set query_label = get_query_options(parse_options=True) -%}
    {%- set full_refresh_build = config.get('full_refresh_build', 'heap_then_index') -%}
    {%- if full_refresh_build == 'prebuilt' -%}
      {{ exceptions.raise_compiler_error(
        "full_refresh_build='prebuilt' is not implemented yet in dbt Core v2's "
        "SQL Server adapter (tracked as a follow-up issue). Use the default 'heap_then_index'."
      ) }}
    {%- elif full_refresh_build != 'heap_then_index' -%}
      {{ exceptions.raise_compiler_error(
        "Invalid full_refresh_build '" ~ full_refresh_build ~ "'. Only 'heap_then_index' (default) is supported."
      ) }}
    {%- endif -%}
    {%- set tmp_relation = relation.incorporate(path={"identifier": relation.identifier ~ '__dbt_tmp_vw'}, type='view') -%}

    {#- Now that the incremental temp build commits standalone (see
        incremental.sql), a crash can leave a throwaway table behind and the
        SELECT * INTO below would hit Msg 2714. Drop it first, but only for
        adapter-generated throwaways: `temporary` covers the incremental
        __dbt_temp build, the suffix covers the full-refresh / table-refresh
        __dbt_tmp intermediate. Suffix match is exact, never substring, so a
        user model named stg__dbt_tmp_x is untouched.
        Never guard a fresh-create of the real target: dbt has decided that
        table does not exist, so 2714 must still surface rather than silently
        destroying an object dbt does not know about. -#}
    {%- set _ident = relation.identifier -%}
    {%- set build_into_temp = temporary or _ident.endswith('__dbt_tmp') or _ident.endswith('__dbt_tmp_vw') -%}

    {%- do adapter.drop_relation(tmp_relation) -%}
    {{ get_use_database_sql(relation.database) }}
    {{ get_create_view_as_sql(tmp_relation, sql) }}

    {%- set table_name -%}
        {{ relation }}
    {%- endset -%}


    {%- set contract_config = config.get('contract') -%}
    {%- set query -%}
        {% if contract_config.enforced and (not temporary) %}
            CREATE TABLE {{table_name}}
            {{ get_assert_columns_equivalent(sql)  }}
            {{ build_columns_constraints(relation) }}
            {% set listColumns %}
                {% for column in model['columns'] %}
                    {{ adapter.quote(column) }}{{ ", " if not loop.last }}
                {% endfor %}
            {%endset%}
            INSERT INTO {{relation}} WITH (TABLOCK) ({{listColumns}})
            SELECT {{listColumns}} FROM {{tmp_relation}} {{ query_label }}

        {% else %}
            {%- if build_into_temp -%}
            IF OBJECT_ID('{{ escape_single_quotes(relation.include(database=False)) }}', 'U') IS NOT NULL
                EXEC('DROP TABLE {{ relation }}');
            {%- endif -%}
            SELECT * INTO {{ table_name }} FROM {{ tmp_relation }} {{ query_label }}
        {% endif %}
    {%- endset -%}

    EXEC('{{- escape_single_quotes(query) -}}')

    {# For some reason drop_relation is not firing. This solves the issue for now. #}
    EXEC('DROP VIEW IF EXISTS {{ tmp_relation.include(database=False) }}')



    {% set as_columnstore = config.get('as_columnstore', default=true) %}
    {% if not temporary and as_columnstore -%}
        {#-
        add columnstore index
        this creates with dbt_temp as its coming from a temporary relation before renaming
        could alter relation to drop the dbt_temp portion if needed
        -#}
        {{ sqlserver__create_clustered_columnstore_index(relation) }}
   {% endif %}

{% endmacro %}
