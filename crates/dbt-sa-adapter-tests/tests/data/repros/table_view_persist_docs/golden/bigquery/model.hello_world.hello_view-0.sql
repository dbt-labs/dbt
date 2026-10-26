SELECT table_catalog,
                    table_schema,
                    table_name,
                    table_type
                FROM `dbt-test-env`.`bigquery_table_view_persist_docs`.INFORMATION_SCHEMA.TABLES
                WHERE table_name = 'hello_view';