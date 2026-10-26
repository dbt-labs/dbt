{% set src_schema = t_schema_name() + '_sources' %}

select id, name, amount from {{ source(src_schema, 'source') }}
