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

`repo` names an extension repository known to DuckDB: a built-in one, such as
`community` or `core_nightly`, or a custom one (see below). Omitting `repo`, or
setting it to `null`, installs from DuckDB's default repository. Repository
names may contain only ASCII letters, digits, `_`, and `-`. URLs and SQL are not
accepted. DuckDB reports an unknown repository name when the extension is
installed.

Mapping entries require a `name` made of ASCII letters, digits, and `_`, and
accept no other fields. String entries keep their existing behavior.

On MotherDuck paths, the `motherduck` extension is still added automatically
unless an entry, in either form, already names it. In MotherDuck SaaS mode, only
`LOAD` runs.

Installing extensions requires an extension-capable DuckDB driver. By default,
dbt loads a DuckDB driver from the system if one is installed, for example
through `dbc`, and otherwise downloads the official DuckDB driver.

## Custom extension repositories

DuckDB 2.0 can install extensions from custom repositories that sign their builds
with their own keys, while still verifying signatures. Declare such a repository
in `extension_repositories` and refer to it from `extensions`:

```yaml
default:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.duckdb
      extension_repositories:
        - name: query_farm
          prefix: https://duckdb-extensions.query.farm
          # Optional: pin the key instead of fetching it on first registration.
          # public_key: "{{ env_var('QUERY_FARM_PUBKEY') }}"
      extensions:
        - httpfs
        - name: a5
          repo: query_farm
```

When the database is initialized, before any extension is installed, dbt makes
sure each declared repository is registered:

- A repository that is not registered yet is created with
  `CREATE EXTENSION REPOSITORY`. Without `public_key`, DuckDB fetches the
  repository's key from the prefix and stores it.
- A repository that is already registered is reused when its prefix matches,
  ignoring a trailing `/`. If `public_key` is set, the stored keys must be exactly
  the configured ones. A different prefix or key is an error. dbt never replaces
  a registration, because that would change which keys DuckDB trusts. Run
  `DROP EXTENSION REPOSITORY "<name>"` to change one.

Extensions from a custom repository are installed and loaded with
`INSTALL <name> FROM "<repo>"` and `LOAD <name> FROM "<repo>"`. A `repo` can also
name a custom repository that was registered earlier without declaring it.

Repository names may contain only ASCII letters, digits, `_`, and `-`, and are
case-insensitive. Built-in names (`core`, `core_nightly`, `community`,
`local_build_debug`, `local_build_release`) cannot be declared. `public_key`
accepts a PEM key, several concatenated PEM keys, or the base64 body of a PEM key.

DuckDB only adds repositories, and installs from them, while
`allow_extension_repositories` is `allowed`. Declaring or using a custom
repository in the profile opts in to them: dbt changes the setting from
`undecided` to `allowed` when it creates a repository or installs from one, and
otherwise leaves it unchanged. If the setting is `forbidden` when the database
is opened, initialization fails, because DuckDB then trusts no custom
repository. `settings` are applied after extensions, so
`allow_extension_repositories: forbidden` in `settings` cannot be combined with
`extension_repositories`.

Repositories are stored in DuckDB's `extension_repository_directory`, by default
`~/.duckdb/extension_repositories`. DuckDB only accepts a different directory when
the database is opened, not in `settings`.

`extension_repositories` requires a DuckDB 2.0 driver. With an older driver,
declaring a repository fails with a compatibility error, and extensions from
built-in repositories keep working.
