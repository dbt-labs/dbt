
{% macro postgres__get_catalog_relations(dbschema, relations) -%}
  {%- call statement('catalog', fetch_result=True) -%}

    {#
      If the user has multiple databases set and the first one is wrong, this will fail.
      But we won't fail in the case where there are multiple quoting-difference-only dbs, which is better.
    #}

    {{ adapter.verify_database(database) }}

    select
        '{{ dbschema.database }}' as table_database,
        sch.nspname as table_schema,
        tbl.relname as table_name,
        case tbl.relkind
            when 'v' then 'VIEW'
            when 'm' then 'MATERIALIZED VIEW'
            else 'BASE TABLE'
        end as table_type,
        tbl_desc.description as table_comment,
        col.attname as column_name,
        col.attnum as column_index,
        pg_catalog.format_type(col.atttypid, col.atttypmod) as column_type,
        col_desc.description as column_comment,
        pg_get_userbyid(tbl.relowner) as table_owner

    from pg_catalog.pg_namespace sch
    join pg_catalog.pg_class tbl on tbl.relnamespace = sch.oid
    join pg_catalog.pg_attribute col on col.attrelid = tbl.oid
    left outer join pg_catalog.pg_description tbl_desc on (tbl_desc.objoid = tbl.oid and tbl_desc.objsubid = 0)
    left outer join pg_catalog.pg_description col_desc on (col_desc.objoid = tbl.oid and col_desc.objsubid = col.attnum)
    where (
      {%- for relation in relations -%}
        {%- if relation.identifier -%}
          (upper(sch.nspname) = upper('{{ relation.schema }}') and
           upper(tbl.relname) = upper('{{ relation.identifier }}'))
        {%- else-%}
          upper(sch.nspname) = upper('{{ relation.schema }}')
        {%- endif -%}
        {%- if not loop.last %} or {% endif -%}
      {%- endfor -%}
    )
      and not pg_is_other_temp_schema(sch.oid) -- not a temporary schema belonging to another session
      and tbl.relpersistence in ('p', 'u') -- [p]ermanent table or [u]nlogged table. Exclude [t]emporary tables
      and tbl.relkind in ('r', 'v', 'f', 'p', 'm') -- o[r]dinary table, [v]iew, [f]oreign table, [p]artitioned table, [m]aterialized view. Other values are [i]ndex, [S]equence, [c]omposite type, [t]OAST table
      and col.attnum > 0 -- negative numbers are used for system columns such as oid
      and not col.attisdropped -- column as not been dropped

    order by
        sch.nspname,
        tbl.relname,
        col.attnum

  {%- endcall -%}

  {{ return(load_result('catalog').table) }}
{%- endmacro %}


{% macro postgres__get_catalog(dbschema, schemas) -%}
  {%- set relations = [] -%}
  {%- for schema in schemas -%}
    {%- set dummy = relations.append({'schema': schema}) -%}
  {%- endfor -%}
  {{ return(postgres__get_catalog_relations(dbschema, relations)) }}
{%- endmacro %}
