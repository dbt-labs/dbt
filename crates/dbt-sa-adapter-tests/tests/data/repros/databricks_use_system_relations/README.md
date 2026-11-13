## Caveats

This test project has 3 major models
- `use_undefined.sql` references a source which schema is not defined in the `databricks/tables.sdf.yml` (compiler function assets)
- `use_defined_directly.sql` and `use_defined.sql` both reference a source which schemas are already defined in the `databricks/tables.sdf.yml`
- the sources referenced by `use_undefined.sql` and `use_defined.sql` are both defined in the project's `sources.yml`
- The difference between `use_defined_directly.sql` from the reset is that the latter references the source using `source` macro that points to a source defined in the `sources.yml`, while the former references the fully qualified relation name directly.

With static analysis on,
- For both `use_defined_directly` and `use_defined.sql`, CLI attempts to download the schemas and hydrate it using the downloaded schemas.
- For `use_defined_directly.sql`, CLI won't be able to download its schema so it'll use what's available in the pre-defined system tables.

> Notes
CLI collects the definitions in `sources` to decide if it will download the sources schemas or not.