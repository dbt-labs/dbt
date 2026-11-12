SELECT table_catalog,
                    table_schema,
                    table_name,
                    table_type
                FROM `dbt-test-env`.`bigquery_execute_project_profile_config`.INFORMATION_SCHEMA.TABLES
                WHERE table_name = 'hello';