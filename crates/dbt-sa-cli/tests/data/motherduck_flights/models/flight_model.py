def model(dbt, session):
    dbt.config(materialized="table", submission_method="flight")
    return dbt.ref("upstream")
