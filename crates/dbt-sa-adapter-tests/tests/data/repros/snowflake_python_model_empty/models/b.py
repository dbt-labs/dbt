# `dbt build --empty` should embed the bare relation
# name into the generated Python `refs` dict, not a run-filter-wrapped SQL
# expression (e.g. `(select * from ... limit 0)`).
def model(dbt, session):
    dbt.config(materialized='table')
    return dbt.ref('a')
