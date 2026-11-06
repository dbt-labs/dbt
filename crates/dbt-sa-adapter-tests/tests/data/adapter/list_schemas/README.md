This is marked as `@available` in bigquery only. 

example run in postgres:

```
data/adapter/list_schemas on  dc/adapter-methods via (env) on ☁️  dev-admin (us-east-1) on ☁️   took 15s 
❯ dbt run
18:26:43  Running with dbt=1.9.2
18:26:43  Registered adapter: postgres=1.9.0
18:26:43  Found 1 model, 433 macros
18:26:43  
18:26:43  Concurrency: 4 threads (target='default')
18:26:43  
18:26:43  1 of 1 START sql view model public.a ........................................... [RUN]
18:26:43  1 of 1 ERROR creating sql view model public.a .................................. [ERROR in 0.01s]
18:26:43  
18:26:43  Finished running 1 view model in 0 hours 0 minutes and 0.11 seconds (0.11s).
18:26:43  
18:26:43  Completed with 1 error, 0 partial successes, and 0 warnings:
18:26:43  
18:26:43    Compilation Error in model a (models/a.sql)
  'dbt.context.providers.RuntimeDatabaseWrapper object' has no attribute 'list_schemas'. This can happen when calling a macro that does not exist. Check for typos and/or install package dependencies with "dbt deps".
18:26:43  
18:26:43  Done. PASS=0 WARN=0 ERROR=1 SKIP=0 TOTAL=1
```