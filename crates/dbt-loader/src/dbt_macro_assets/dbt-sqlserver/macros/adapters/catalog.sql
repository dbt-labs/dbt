{% macro sqlserver__get_catalog(information_schemas, schemas) -%}
    {% set query_label = get_query_options() %}
    {%- call statement('catalog', fetch_result=True) -%}
        {{ get_use_database_sql(information_schemas.database) }}
        with
        principals as (
            select
                name as principal_name,
                principal_id as principal_id
            from
                sys.database_principals {{ information_schema_hints() }}
        ),

        schemas as (
            select
                name as schema_name,
                schema_id as schema_id,
                principal_id as principal_id
            from
                sys.schemas {{ information_schema_hints() }}
        ),

        tables as (
            select
                t.object_id,
                t.name as table_name,
                t.schema_id as schema_id,
                t.principal_id as principal_id,
                'BASE TABLE' as table_type,
                cast(ep.value as nvarchar(max)) as table_comment
            from
                sys.tables as t {{ information_schema_hints() }}
            left join sys.extended_properties as ep {{ information_schema_hints() }}
                on ep.class = 1
               and ep.major_id = t.object_id
               and ep.minor_id = 0
               and ep.name = N'MS_Description'
        ),

        tables_with_metadata as (
            select
                object_id,
                table_name,
                schema_name,
                coalesce(tables.principal_id, schemas.principal_id) as owner_principal_id,
                table_type,
                table_comment
            from
                tables
            join schemas on tables.schema_id = schemas.schema_id
        ),

        views as (
            select
                v.object_id,
                v.name as table_name,
                v.schema_id as schema_id,
                v.principal_id as principal_id,
                'VIEW' as table_type,
                cast(ep.value as nvarchar(max)) as table_comment
            from
                sys.views as v {{ information_schema_hints() }}
            left join sys.extended_properties as ep {{ information_schema_hints() }}
                on ep.class = 1
               and ep.major_id = v.object_id
               and ep.minor_id = 0
               and ep.name = N'MS_Description'
        ),

        views_with_metadata as (
            select
                object_id,
                table_name,
                schema_name,
                coalesce(views.principal_id, schemas.principal_id) as owner_principal_id,
                table_type,
                table_comment
            from
                views
            join schemas on views.schema_id = schemas.schema_id
        ),

        tables_and_views as (
            select
                object_id,
                table_name,
                schema_name,
                principal_name,
                table_type,
                table_comment
            from
                tables_with_metadata
            join principals on tables_with_metadata.owner_principal_id = principals.principal_id
            union all
            select
                object_id,
                table_name,
                schema_name,
                principal_name,
                table_type,
                table_comment
            from
                views_with_metadata
            join principals on views_with_metadata.owner_principal_id = principals.principal_id
        ),

        cols as (
            select
                c.object_id,
                c.name as column_name,
                c.column_id as column_index,
                t.name as column_type,
                cast(ep.value as nvarchar(max)) as column_comment
            from sys.columns as c {{ information_schema_hints() }}
            left join sys.types as t {{ information_schema_hints() }}
                on c.user_type_id = t.user_type_id
            left join sys.extended_properties as ep {{ information_schema_hints() }}
                on ep.class = 1
               and ep.major_id = c.object_id
               and ep.minor_id = c.column_id
               and ep.name = N'MS_Description'
        )

        select
            DB_NAME() as table_database,
            tv.schema_name as table_schema,
            tv.table_name,
            tv.table_type,
            tv.table_comment,
            tv.principal_name as table_owner,
            cols.column_name,
            cols.column_index,
            cols.column_type,
            cols.column_comment
        from tables_and_views tv
        join cols on tv.object_id = cols.object_id
        where ({%- for schema in schemas -%}
            upper(tv.schema_name) = upper('{{ schema }}'){%- if not loop.last %} or {% endif -%}
        {%- endfor -%})

        order by column_index
        {{ query_label }}

        {%- endcall -%}

    {{ return(load_result('catalog').table) }}

{%- endmacro %}

{% macro sqlserver__get_catalog_relations(information_schema, relations) -%}
    {% set query_label = get_query_options() %}
    {%- set distinct_databases = relations | map(attribute='database') | unique | list -%}

    {%- if distinct_databases | length == 1 -%}
        {%- call statement('catalog', fetch_result=True) -%}
            {{ get_use_database_sql(distinct_databases[0]) }}
            with
            principals as (
                select
                    name as principal_name,
                    principal_id as principal_id
                from
                    sys.database_principals {{ information_schema_hints() }}
            ),

            schemas as (
                select
                    name as schema_name,
                    schema_id as schema_id,
                    principal_id as principal_id
                from
                    sys.schemas {{ information_schema_hints() }}
            ),

            tables as (
                select
                    t.object_id,
                    t.name as table_name,
                    t.schema_id as schema_id,
                    t.principal_id as principal_id,
                    'BASE TABLE' as table_type,
                    cast(ep.value as nvarchar(max)) as table_comment
                from
                    sys.tables as t {{ information_schema_hints() }}
                left join sys.extended_properties as ep {{ information_schema_hints() }}
                    on ep.class = 1
                   and ep.major_id = t.object_id
                   and ep.minor_id = 0
                   and ep.name = N'MS_Description'
            ),

            tables_with_metadata as (
                select
                    object_id,
                    table_name,
                    schema_name,
                    coalesce(tables.principal_id, schemas.principal_id) as owner_principal_id,
                    table_type,
                    table_comment
                from
                    tables
                join schemas on tables.schema_id = schemas.schema_id
            ),

            views as (
                select
                    v.object_id,
                    v.name as table_name,
                    v.schema_id as schema_id,
                    v.principal_id as principal_id,
                    'VIEW' as table_type,
                    cast(ep.value as nvarchar(max)) as table_comment
                from
                    sys.views as v {{ information_schema_hints() }}
                left join sys.extended_properties as ep {{ information_schema_hints() }}
                    on ep.class = 1
                   and ep.major_id = v.object_id
                   and ep.minor_id = 0
                   and ep.name = N'MS_Description'
            ),

            views_with_metadata as (
                select
                    object_id,
                    table_name,
                    schema_name,
                    coalesce(views.principal_id, schemas.principal_id) as owner_principal_id,
                    table_type,
                    table_comment
                from
                    views
                join schemas on views.schema_id = schemas.schema_id
            ),

            tables_and_views as (
                select
                    object_id,
                    table_name,
                    schema_name,
                    principal_name,
                    table_type,
                    table_comment
                from
                    tables_with_metadata
                join principals on tables_with_metadata.owner_principal_id = principals.principal_id
                union all
                select
                    object_id,
                    table_name,
                    schema_name,
                    principal_name,
                    table_type,
                    table_comment
                from
                    views_with_metadata
                join principals on views_with_metadata.owner_principal_id = principals.principal_id
            ),

            cols as (
                select
                    c.object_id,
                    c.name as column_name,
                    c.column_id as column_index,
                    t.name as column_type,
                    cast(ep.value as nvarchar(max)) as column_comment
                from sys.columns as c {{ information_schema_hints() }}
                left join sys.types as t {{ information_schema_hints() }}
                    on c.user_type_id = t.user_type_id
                left join sys.extended_properties as ep {{ information_schema_hints() }}
                    on ep.class = 1
                and ep.major_id = c.object_id
                and ep.minor_id = c.column_id
                and ep.name = N'MS_Description'
            )

            select
                DB_NAME() as table_database,
                tv.schema_name as table_schema,
                tv.table_name,
                tv.table_type,
                tv.table_comment,
                tv.principal_name as table_owner,
                cols.column_name,
                cols.column_index,
                cols.column_type,
                cols.column_comment
            from tables_and_views tv
            join cols on tv.object_id = cols.object_id
            where (
                {%- for relation in relations -%}
                    {% if relation.schema and relation.identifier %}
                        (
                            upper(tv.schema_name) = upper('{{ relation.schema }}')
                            and upper(tv.table_name) = upper('{{ relation.identifier }}')
                        )
                    {% elif relation.schema %}
                        (
                            upper(tv.schema_name) = upper('{{ relation.schema }}')
                        )
                    {% else %}
                        {% do exceptions.raise_compiler_error(
                            '`get_catalog_relations` requires a list of relations, each with a schema'
                        ) %}
                    {% endif %}

                    {%- if not loop.last %} or {% endif -%}
                {%- endfor -%}
            )

            order by column_index
            {{ query_label }}

        {%- endcall -%}
        {{ return(load_result('catalog').table) }}
    {% else %}
        {% do exceptions.raise_compiler_error(
            '`get_catalog_relations` can catalog one database at a time'
        ) %}
    {% endif %}

{%- endmacro %}
