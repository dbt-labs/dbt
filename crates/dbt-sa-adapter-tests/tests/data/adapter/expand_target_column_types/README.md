
## Prerequisites
- Local instance of Postgres running on `localhost` with the standard port number
    - `fusion` user and `magic` database in Postgres
- Default connection in `profiles.yml` works
- These interfaces are implemented for the adapter:
    - `expand_target_column_types`
    - `alter_column_type`

See Appendix for further guidance.

## Run

```shell
dbt build
```

## Expected output

Check the logs and see that `ALTER` statements have occurred that change the data type of the column.

Alternatively, run `dbt docs generate` and examine `catalog.json` and see that:
- model `b` has datatype `varchar(2)` (orignally `varchar(1)`)
- model `c` has datatype `varchar(3)` (orignally `varchar(1)`)

## Appendix

### Setting up postgres user

Create the user and database:
```sql
CREATE USER fusion WITH PASSWORD 'rocks';
CREATE DATABASE magic;
GRANT ALL PRIVILEGES ON DATABASE magic TO fusion;
```

Then create a new connection using the `magic` database and run the following:

```sql
GRANT USAGE, CREATE ON SCHEMA public TO fusion;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO fusion;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO fusion;
```

### Test the connection

```shell
dbt debug
```
