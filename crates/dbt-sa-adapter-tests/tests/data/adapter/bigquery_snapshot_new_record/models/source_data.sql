{{ config(materialized='table') }}

{#- Snapshot source with a STRUCT (`contact`) and an ARRAY (`tags`) column;
    `drop_rows` hard-deletes ids 4-5 between snapshots. -#}
select
    id,
    first_name,
    struct(first_name as first, last_name as last) as contact,
    [gender, ip_address] as tags,
    updated_at
from (
    select 1 as id, 'Judith' as first_name, 'Kennedy' as last_name, 'Female' as gender, '54.60.24.128' as ip_address, timestamp('2015-12-24') as updated_at union all
    select 2, 'Arthur', 'Kelly', 'Male', '62.56.24.215', timestamp('2015-10-28') union all
    select 3, 'Rachel', 'Moreno', 'Female', '31.222.249.23', timestamp('2016-04-05') union all
    select 4, 'Ralph', 'Turner', 'Male', '157.83.76.114', timestamp('2016-08-08') union all
    select 5, 'Laura', 'Gonzales', 'Female', '30.54.105.168', timestamp('2016-09-01')
)
{%- if var('drop_rows', false) %}
where id <= 3
{%- endif %}
