
## Prerequisites
- Local instance of Postgres running on `localhost` with the standard port number
    - `fusion` user and `magic` database in Postgres
- Default connection in `profiles.yml` works

See Appendix for further guidance.

## Run

```shell
dbt build
```

## Expected output

```
True: target.schema exists
False: nonexistent_schema exists
```

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

