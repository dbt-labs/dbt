{{ config(
    materialized='table',
    labels={'contains_pii': 'no'},
) }}

select 1 as id, "foo" as name
