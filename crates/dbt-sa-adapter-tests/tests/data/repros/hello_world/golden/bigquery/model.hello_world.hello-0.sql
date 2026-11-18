SELECT table_catalog,
                    table_schema,
                    table_name,
                    table_type
                FROM `dbt-test-env`.`bigquery_create_table_run_twice`.INFORMATION_SCHEMA.TABLES
                WHERE table_name = 'hello';