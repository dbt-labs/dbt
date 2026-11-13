`verify_adapters` is an adapter method to ensure that the configured database and the configured RA3 node are the same. 

This method returns a compiler error if the configured database for a model and the database from your profile are different *unless you have set `ra3_node: true` [in your profile](https://docs.getdbt.com/docs/core/connect-data-platform/redshift-setup#configurations)*

This project has three profiles -- postgres, redshift and redshift with ra3 node. The project will fail to compile with 2 out of the three. 

### Expected output

| dbt command                              | Expected output                                                                                                      | Exit Code |
|------------------------------------------|----------------------------------------------------------------------------------------------------------------------|-----------|
| `dbt run --target postgres`              | Compilation Error in model b (models/b.sql)   ERROR: Cross-db references not allowed in postgres (other vs postgres) | 2         |
| `dbt run --target redshift_no_ra3_node`  | Compilation Error in model b (models/b.sql)   ERROR: Cross-db references allowed only in RA3.* node. (other vs ci)   | 2         |
| `dbt run --target redshift_ra3_node`     | Completed successfully                                                                                               | 0         |