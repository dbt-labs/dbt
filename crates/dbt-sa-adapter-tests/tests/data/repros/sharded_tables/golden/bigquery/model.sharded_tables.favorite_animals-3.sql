

  create or replace view `dbt-test-env`.`adapters_bigquery_repros_sharded_tables`.`favorite_animals`
  OPTIONS()
  as 

select * from `dbt-test-env`.`adapters_bigquery_repros_sharded_tables`.`favourite_animals_*`;

