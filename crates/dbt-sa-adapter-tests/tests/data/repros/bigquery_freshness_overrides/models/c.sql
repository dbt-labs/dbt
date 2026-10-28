{% set src_schema = t_schema_name() + '_sources' %}

select id, updated_at from {{ source(src_schema, 'source_bulk') }}
union all
select id, updated_at from {{ source(src_schema, 'source_field') }}
union all
select id, updated_at from {{ source(src_schema, 'source_query') }}
