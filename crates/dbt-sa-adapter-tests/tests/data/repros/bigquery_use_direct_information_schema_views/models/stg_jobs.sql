{{
    config(
        materialized='table'
    )
}} 

-- https://cloud.google.com/bigquery/docs/information-schema-jobs
select * from `dbt-test-env`.`region-us`.INFORMATION_SCHEMA.JOBS limit 1
