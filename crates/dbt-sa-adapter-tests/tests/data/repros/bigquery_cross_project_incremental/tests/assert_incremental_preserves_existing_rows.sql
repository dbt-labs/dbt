with row_counts as (
    select
        count(*) as total_rows,
        countif(id = 1) as initial_rows,
        countif(id = 2) as incremental_rows
    from {{ ref('incremental_model') }}
)

select *
from row_counts
where total_rows != 2
    or initial_rows != 1
    or incremental_rows != 1
