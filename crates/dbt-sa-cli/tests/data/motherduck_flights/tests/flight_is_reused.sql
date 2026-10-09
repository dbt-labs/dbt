{% do ref('flight_model') %}
{% set flight_name = 'dbt-motherduck_flights-' ~ target.database ~ '-' ~ target.schema ~ '-flight_model' %}

with matching as (
    select flight_id, current_version
    from MD_LIST_FLIGHTS("limit" := 200, owner_only := true)
    where flight_name = '{{ flight_name | replace("'", "''") }}'
)
select flight_id, current_version
from matching
where current_version <> 1
union all
select null, null
where (select count(*) from matching) <> 1
