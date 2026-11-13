{% set src_schema = t_schema_name() + '_sources' %}

select * from {{ source(src_schema, 'source') }}
