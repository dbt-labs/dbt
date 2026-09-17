# Databricks Unity Catalog reads on Lake Compute

Lake Compute can read AWS Databricks Unity Catalog tables declared in
`catalogs.yml`. The catalog entry's `catalog_database` is the name used in
SQL; the `catalogs.yml` entry `name` is only the configuration name.

```yaml
catalogs:
  - name: dbx_raw
    type: unity
    table_format: iceberg
    config:
      lakecompute:
        catalog_database: raw
        region: us-east-1
        host: dbc-example.cloud.databricks.com
```

The `config.lakecompute` block must contain `catalog_database` and `region`.
The region is the AWS region where the Unity Catalog table storage lives; use
the workspace's home region for the normal single-region setup. Multi-region
storage under one Unity Catalog is not currently verified.

## Credentials

Credentials must be configured in the Databricks profile connection, not in
`catalogs.yml`. A target may keep Snowflake as its default connection and add
Databricks as a secondary connection:

```yaml
outputs:
  default:
    - type: snowflake
      default: true
      # Snowflake connection fields...
    - type: databricks
      host: dbc-example.cloud.databricks.com
      http_path: "{{ env_var('DATABRICKS_HTTP_PATH') }}"
      token: "{{ env_var('DBX_TOKEN') }}"
```

The Databricks profile connection requires an `http_path` for a SQL warehouse
or cluster and may use either a PAT (`token`) or an OAuth M2M pair (`client_id`
and `client_secret`). Hosts must be HTTPS Databricks AWS
workspace hosts ending in `.cloud.databricks.com`; non-default ports, paths, query
strings, fragments, userinfo, GCP hosts, and other domains are rejected. An
explicit HTTPS port (`:443`) is accepted and normalized away. Azure-shaped
profile credentials are not used for AWS Unity reads.

Unity entries are self-contained read connections, and are only used for
AWS Databricks workspaces. Azure and GCP Unity reads are unsupported. A Lake
Compute request may combine a Databricks Unity catalog with Snowflake-backed
catalog attachment types; the Unity entry supplies its Databricks credentials
while the Snowflake bundle credential handles Snowflake attachments or
propagation.

## Local attach migration

`config.lakecompute` on Unity now describes the Lake Compute read connection.
Move old local DuckDB/Iceberg attach options such as `endpoint`, `warehouse`,
`secret`, `read_only`, and write-compatibility ATTACH options to
`config.duckdb`. `catalog_database` is valid in both blocks and does not need
to move. This migration preserves local Unity attach behavior, including
`read_only: false` local writes when the installed DuckDB version supports the
Unity write-compatibility options. The Lake Compute relation check is separate:
Unity model targets are read-only in this release because propagation to Unity
is deferred.

## Addressing and limitations

A model reads the real Unity catalog name:

```sql
select * from raw.analytics.orders
```

It does not use `dbx_raw` unless that is also the Unity catalog's
`catalog_database` value. The analyzer registers referenced tables and the
worker attaches Unity read-only; plain Delta, UniForm Delta, and native Iceberg
follow the same fs bundle shape.

The pinned Unity Catalog DuckDB extension does not support `TIMESTAMP_NTZ`
columns. Use timezone-aware `TIMESTAMP` or compatible primitive columns for
Lake Compute queries until the extension limitation is removed.
