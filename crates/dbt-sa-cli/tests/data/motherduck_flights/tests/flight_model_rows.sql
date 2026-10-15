with actual as (
    select grp, count(*) as row_count
    from {{ ref('flight_model') }}
    group by grp
),
expected(grp, row_count) as (
    values (0, 33), (1, 34), (2, 33)
)
(
    select * from actual
    except
    select * from expected
)
union all
(
    select * from expected
    except
    select * from actual
)
