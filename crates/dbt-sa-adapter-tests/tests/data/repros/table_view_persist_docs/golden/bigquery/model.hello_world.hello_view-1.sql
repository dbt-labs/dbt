create or replace view `dbt-test-env`.`bigquery_table_view_persist_docs`.`hello_view`
  OPTIONS(
      description="""The view description"""
    )
  as 

select * from `dbt-test-env`.`bigquery_table_view_persist_docs`.`hello`