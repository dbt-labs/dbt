with source_data as (
    select 1 as id, 'John' as name, 'john@example.com' as email, 100.50 as amount
    union all select 2 as id, 'Jane' as name, 'jane@example.com' as email, 200.75 as amount
    union all select 3 as id, 'Bob' as name, 'bob@example.com' as email, 150.25 as amount
)

select id
       ,name
       ,email
       ,amount

from source_data

