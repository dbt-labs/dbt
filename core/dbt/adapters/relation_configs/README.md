This package contains classes to model the database objects as they
are described by the database. They exist primarily as an in between
to parse node configuration from the user and database configuration
in database-specific terms. For example, we expose `method` for indexes
in Postgres as `type`. We want to retain that term across adapters, but
it's more useful to call it type in Postgres parlance. Similarly,
we combine `distkey` and `diststyle` in Redshift into a single `dist`
config. This makes sense to the end user because of the available
combinations of these two terms (if `diststyle` is `key`, then `distkey`
is used; if `diststyle` is not `key`, then `distkey` is not used,
hence if `dist` is not one of the other `diststyle`, it's assumed it's
a `distkey` and `diststyle` is `key`. This kind of nuance can be
parsed out in these class.

A secondary reason for this package is to place some governance on how
configuration changes are handled in `dbt`. Until recently, changes
have been handled via DROP/CREATE. However, we are going to start
applying changes to existing objects. Given this is new functionality,
it makes sense to place this in a new subpackage.
