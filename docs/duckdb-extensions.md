# DuckDB extensions

`extensions` in a DuckDB profile lists extensions to install and load when the
database is initialized, before secrets, settings, and attachments:

```yaml
default:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.duckdb
      extensions:
        - httpfs
        - name: parquet
        - name: h3
          repo: community
```

An entry is either an extension name or a mapping with `name` and an optional
`repo`. The example above runs:

```sql
INSTALL httpfs;
LOAD httpfs;
INSTALL parquet;
LOAD parquet;
INSTALL h3 FROM "community";
LOAD h3;
```

`repo` names an extension repository known to DuckDB, such as `community` or
`core_nightly`. Omitting `repo`, or setting it to `null`, installs from DuckDB's
default repository. Repository names may contain only ASCII letters, digits, `_`,
and `-`. URLs and SQL are not accepted. DuckDB reports an unknown repository
name when the extension is installed.

Mapping entries require a `name` made of ASCII letters, digits, and `_`, and
accept no other fields. String entries keep their existing behavior.

On MotherDuck paths, the `motherduck` extension is still added automatically
unless an entry, in either form, already names it. In MotherDuck SaaS mode, only
`LOAD` runs, so `repo` has no effect there.

Installing extensions requires an extension-capable DuckDB driver. By default,
dbt loads a DuckDB driver from the system if one is installed, for example
through `dbc`, and otherwise downloads the official DuckDB driver.
