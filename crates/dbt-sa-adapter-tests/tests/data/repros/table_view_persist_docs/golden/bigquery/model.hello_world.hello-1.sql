create or replace table `dbt-test-env`.`bigquery_table_view_persist_docs`.`hello`
      
    
    

    OPTIONS(
      description="""The table description"""
    )
    as (
      

select 1 as id, "foo" as name
    )