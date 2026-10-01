# DuckDB v2 catalog ATTACH snapshot fixtures

Each subdirectory under this folder is one snapshot test case for
`compose_v2_catalog_attach_stmts`:

```
fixtures/
└── <scenario>/
    ├── catalogs.yml   ← input
    └── output.snap    ← expected ATTACH (+ optional INSTALL ducklake) output,
                         managed by insta
```

Side-by-side layout makes diffs reviewable in one place: when a fixture's
output changes, the YAML and the snapshot show up next to each other in the
PR diff.

## Convention

- Lead each `catalogs.yml` with a YAML comment describing the scenario:
  ```yaml
  # Scenario: <short description>
  # Exercises: <which behavior this case is meant to lock in>
  ```
- Directory name = test case name. Use `snake_case`.
- Error cases are named `<thing>_error`.
- The snapshot is always called `output.snap`. The harness suppresses
  insta's per-glob suffix so each scenario dir contains exactly one snap.

## Workflow

- Run: `cargo xtask test --llm --no-external-deps -p dbt-adapter duckdb_attach_fixtures`
- After an intentional change: `cargo insta review` (or `cargo insta accept`).
- Add a case: create a new subdirectory with `catalogs.yml`, run the test
  (it will fail with a missing snapshot), then `cargo insta accept`.
