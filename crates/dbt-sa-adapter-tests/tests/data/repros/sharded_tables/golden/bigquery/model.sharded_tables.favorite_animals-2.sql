SELECT table_catalog,
                    table_schema,
                    table_name,
                    table_type
                FROM `dbt-test-env`.`adapters_bigquery_repros_sharded_tables`.INFORMATION_SCHEMA.TABLES
                WHERE table_name = 'favorite_animals';