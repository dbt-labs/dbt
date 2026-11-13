# adapters_redshift_use_transactions

With `redshift_use_transactions: true`, each node runs in one transaction, as in dbt-redshift:
`BEGIN` before the first statement that writes, `COMMIT` at `adapter.commit()`.

Both models write relation and column docs, so each node sends several statements after its
main one. Read `golden/redshift/run_*/recordings.yaml` to see the `begin` and `commit`
statements around each node's work.

The first run creates both relations. The second run takes the incremental path.
