{% macro redshift__get_relations() -%}

{%- call statement('relations', fetch_result=True) -%}

with
    relation as (
        select
            pg_class.oid as relation_id,
            pg_class.relname as relation_name,
            pg_class.relnamespace as schema_id,
            pg_namespace.nspname as schema_name,
            pg_class.relkind as relation_type
        from pg_class
        join pg_namespace
          on pg_class.relnamespace = pg_namespace.oid
        where pg_namespace.nspname != 'information_schema'
          and pg_namespace.nspname not like 'pg\_%'
    ),
    dependency as (
        select distinct
            coalesce(pg_rewrite.ev_class, pg_depend.objid) as dep_relation_id,
            pg_depend.refobjid as ref_relation_id,
            pg_depend.refclassid as ref_class_id
        from pg_depend
        left join pg_rewrite
          on pg_depend.objid = pg_rewrite.oid
        where coalesce(pg_rewrite.ev_class, pg_depend.objid) != pg_depend.refobjid
    )

select distinct
    dep.schema_name as dependent_schema,
    dep.relation_name as dependent_name,
    ref.schema_name as referenced_schema,
    ref.relation_name as referenced_name
from dependency
join relation ref
    on dependency.ref_relation_id = ref.relation_id
join relation dep
    on dependency.dep_relation_id = dep.relation_id

{%- endcall -%}

{{ return(load_result('relations').table) }}

{% endmacro %}
