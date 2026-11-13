# redshift_transaction_perf

Measures what `redshift_use_transactions` changes on Redshift, and checks that it does not leak
transactions. Run it by hand against a Redshift cluster. It is not a record/replay test.

## Why

Fusion commits every statement on its own. Redshift commits one at a time, so concurrent nodes
queue behind each other's commits. A node with `persist_docs` sends one `comment on` statement per
column, so it pays for each one. dbt-redshift runs each node in one transaction and commits once.

With `redshift_use_transactions: true`, Fusion does the same. This directory has the tools that
measured the effect and audited the change.

## Files

| File | Purpose |
| --- | --- |
| `gen_project.py` | Writes the project variants: 48 incremental models, 31 documented columns each, `persist_docs` on. |
| `run_experiment.py` | Runs the variants in randomized blocks and records per-node times, per-statement times and commit-queue stats. |
| `analyze.py` | Summarizes the results and compares each arm with the reference arm, block by block. |
| `audit_transactions.py` | Checks that every transaction ends and that committed work is visible from another session. |
| `common.py` | Profile loading and connections. |

## Arms

| Arm | What runs |
| --- | --- |
| `A` | Fusion with default settings. |
| `B` | Fusion with `redshift_use_transactions: true`. |
| `E` | Fusion with default settings, plus a macro that wraps only the column comments in one transaction. It shows how much of the gain that narrow change gives. |
| `P` | A second Fusion binary (`--pristine-bin`) on project A. Use a build without your change to confirm that arm A matches it. |
| `V1` | Python dbt-redshift (`--v1-dbt`) on project A, as the reference. |

A, B and E use one Fusion binary, so only the project differs. Each block runs every arm once, in
random order, so slow changes on a shared cluster affect all arms alike.

## Prerequisites

- Python 3.10 or later, with `redshift_connector` and `pyyaml`. Add `scipy` for the p-values.
- A `fusion_tests` profile with a `redshift` output in `~/.dbt/profiles.yml`
  (or in `$DBT_PROFILES_DIR`). Its user must be able to create and drop schemas.
- A Fusion binary that has the flag. A release build gives realistic client timings at 64 threads.
- For arm `V1`: a separate virtual environment with `dbt-core` and `dbt-redshift`.

```
python3 -m venv .venv
.venv/bin/pip install redshift_connector pyyaml scipy
python3 -m venv .venv-v1
.venv-v1/bin/pip install dbt-core dbt-redshift
```

The scripts write a temporary `profiles.yml` that points at a scratch schema and holds the
password. They delete it when they finish. They create and drop the scratch schemas
`rca_txn_exp`, `rca_audit` and `rca_audit_extra`. Do not run them against a cluster where those
names matter.

## Run the audit

```
.venv/bin/python audit_transactions.py --bin /path/to/dbt
```

It runs `build`, `snapshot` and `build --full-refresh` with the setting off, then on. For each
command it reads `query_log.sql` and checks that `begin` equals `commit` plus `rollback`. Then it
checks from a separate session that all eight expected objects exist. The setting-off run is the
control: it must show no `begin`, `commit` or `rollback`. The script exits 1 if anything fails.

A passing run:

```
[on] build                  rc=0 Summary: 12 total | 12 success  begin=10 commit=8 rollback=2  paired
[on] visible from another session: 8/8; missing=[]
AUDIT PASSED
```

## Run the experiment

A quick check that everything works (a few minutes):

```
.venv/bin/python run_experiment.py --v2-bin /path/to/dbt --arms A,B,E \
  --blocks 1 --models 8 --threads 8 --out smoke.jsonl
.venv/bin/python analyze.py smoke.jsonl
```

The full comparison (about 10 minutes per block with five arms at 64 threads):

```
.venv/bin/python run_experiment.py --v2-bin /path/to/dbt \
  --pristine-bin /path/to/dbt-without-the-change --v1-dbt .venv-v1/bin/dbt \
  --arms A,B,E,P,V1 --blocks 6 --threads 64 --out results.jsonl
.venv/bin/python analyze.py results.jsonl A
```

The first run creates the tables and is not measured. Pass `--skip-setup` to reuse them. Run it
when the cluster is quiet, and do not build or test on the same machine while it runs. The client
needs its CPU at 64 threads.

## Read the results

`analyze.py` prints, for each arm, the median over clean runs of:

- `job wall`: the time of the whole `dbt run`.
- `node mean` and `node p95`: the time of each node, from `run_results.json`.
- `commits` and `queue ms`: commits on the whole cluster during the run, and their mean wait in
  the commit queue, from `stl_commit_stats`.

It then compares each arm's per-node mean with the reference arm in the same block. A ratio below
1 is faster. The p-value comes from a Wilcoxon signed-rank test. With n blocks the smallest
possible two-sided p is `2 / 2**n`: 0.0625 for 5 blocks, 0.0312 for 6. Use at least 6 blocks if
you want to see p below 0.05. It tells you only that the arms never swapped order.

The last lines list the mean of the `comment on` and column-lookup statements, and how many other
sessions were on the cluster before each run. A high session count means a noisy cluster. Treat
absolute times from such a run with care; compare arms within a block.

A run counts as clean when `dbt` exits 0 and every node succeeded. The others are listed first,
with the end of their output.

## Reference results

64 threads, 48 models, 6 blocks, a release build, a shared sandbox cluster, 2026-11-12. Arm values
are medians over blocks.

| Arm | Job wall | Per-node mean | Against A | `comment on` | Commits |
| --- | --- | --- | --- | --- | --- |
| A | 118.6 s | 105.6 s | 1.00 | 1.19 s | 3,388 |
| B | 81.1 s | 69.4 s | 0.65 (p=0.031) | 0.14 s | 257 |
| E | 81.0 s | 69.4 s | 0.66 (p=0.031) | 0.16 s | 399 |
| V1 | 82.2 s | 69.9 s | 0.66 (p=0.031) | n/a | 250 |

A second run of arms A, B and V1 on the final binary (5 blocks) gave 123.9 s, 87.3 s and 85.5 s.

Column lookups are not part of the gain. A node runs five of them, and each took about 9 s at
64 threads and 0.65 s at 5 threads, with or without a transaction. That cost depends on how many
nodes run at once, and dbt-redshift pays it too.

## Limits

- Concurrent transactions can fail with `conflict with concurrent transaction` on a busy cluster.
  dbt-redshift can too. Fusion fails the node and does not retry.
- The scripts measure one workload: incremental models with `persist_docs`. Other node types and
  other statement mixes may gain more or less.
- Arm `V1` uses whatever dbt-redshift version you install.
- If a run is interrupted, drop the scratch schemas by hand.
