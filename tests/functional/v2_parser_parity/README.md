# v2 parser parity

A pytest plugin that re-runs selected functional tests through the v2
parser dispatch path, using dbt-core itself as the "v2 parser"
binary. The goal is to surface hidden parse-phase state: anything that
breaks when later phases consume `manifest.json` instead of the
in-memory `Manifest` that `ManifestLoader` builds.

## Why

The v2 parser flow (`core/dbt/parser/v2.py::parse_with_v2`) shells
out to the v2 parser, reads the resulting `manifest.json`, hydrates
a `Manifest` via `Manifest.from_writable_manifest`, and hands that to
compile/run/etc. The classic dbt parse flow builds `Manifest` directly in
memory and never serializes through `manifest.json`. Any attribute that
`ManifestLoader` populates but doesn't survive the
`WritableManifest → Manifest` round-trip is a latent bug for the v2 flow.

Running the real v2 parser binary in CI is heavy and only validates
what it chooses to emit. By substituting in-process `dbt parse` for the
external parser, we keep the `parse_with_v2` machinery (handoff dir,
`WritableManifest` load, `build_flat_graph`, `partial_parse` cleanup) but
exercise it against a parser whose output we control. If a downstream
phase misbehaves, the divergence is between in-memory `Manifest` and
`manifest.json` — not between dbt and the external parser.

This trades subprocess fidelity for breadth. It will not catch bugs in
`_build_argv` flag translation or in the external parser binary itself.
Pair it with a small set of real subprocess tests (see
`tests/functional/v2_parser/`) for argv coverage.

## How it works

Three pieces:

1. **The shim** (`v2_self_parser.py::install_shim`). Monkeypatches
   `dbt.parser.v2._run_v2` so it invokes an in-process
   `run_dbt(["parse"])` whose `manifest.json` lands in the same handoff
   directory `parse_with_v2` created. Also stubs the two plugin
   guards (`assert_no_get_nodes_plugins`,
   `enrich_manifest_with_plugin_artifacts`) since Mantle-registered
   `get_nodes` plugins fail fast on the v2 branch by design and
   these tests aren't about plugin interop.

2. **The pytest plugin** (`plugin.py`). Adds a `--v2-parser-parity` CLI
   flag, registers a `v2_parser_parity` marker, and parametrizes a
   `parser_mode` fixture. Tests carrying the marker run twice
   (`[core]` + `[v2_self]`) when the flag is set, once (`[core]`)
   otherwise. The `parser_mode` fixture installs the shim for
   `v2_self` runs and tears it down via pytest's `monkeypatch`.

3. **The helpers** (`v2_self_parser.py::run_dbt_for_mode`,
   `run_dbt_and_capture_for_mode`). Drop-in replacements for
   `run_dbt` / `run_dbt_and_capture` that prepend
   `--use-v2-parser --v2-parser=dbt parse` when the mode is
   `v2_self`. The `--v2-parser` value is a placeholder — the
   shim replaces `_run_v2`, so the command is never executed, but
   the CLI flag pair is required for `USE_V2_PARSER` validation.

## Adopting the marker on a test

1. Replace `run_dbt` / `run_dbt_and_capture` imports with
   `run_dbt_for_mode` / `run_dbt_and_capture_for_mode` from this package.
2. Add `parser_mode` to the test signature.
3. Add `@pytest.mark.v2_parser_parity`.

```python
from tests.functional.v2_parser_parity.v2_self_parser import run_dbt_for_mode

@pytest.mark.v2_parser_parity
def test_my_thing(project, parser_mode):
    results = run_dbt_for_mode(parser_mode, ["run"])
    ...
```

Run the suite normally for `[core]`-only coverage:

```
pytest tests/functional/basic/
```

Run with `--v2-parser-parity` to add the `[v2_self]` variant:

```
pytest tests/functional/basic/ --v2-parser-parity
```

## What's eligible

Tests that exercise parse → later-phase data flow are the target. Tests
that fail before `parse_with_v2` is dispatched (e.g.
`ProjectContractError` from `dbt_project.yml` validation) gain nothing
from parity coverage — the v2 flag never takes effect. Tests that use
`dbtRunner.invoke` directly bypass the helper and need their own
wrapping if you want them under the marker.

When in doubt: if the test only calls `run_dbt(["parse"|"compile"|
"run"|"build"|"seed"|"test"|"snapshot"])` and asserts on its results
or on the resulting database state / manifest, it's a candidate.

## Findings

### Resolved in this PR — `get_manifest()` returning None

`dbt.tests.util.get_manifest` previously read only
`target/partial_parse.msgpack` and returned `None` if absent.
`parse_with_v2` deletes that msgpack after the handoff (see
`v2.py::_delete_stale_partial_parse`, intentional — prevents
later runs from picking up a stale msgpack that wouldn't reflect the
v2 handoff). That left `get_manifest()` with nothing to read.

Resolved by extending `get_manifest()` to fall back to
`target/manifest.json` via `WritableManifest.read_and_check_versions`
and `Manifest.from_writable_manifest` — the same load path
`parse_with_v2` uses. The helper has no non-test callers, so the
change is scoped to test infrastructure.

### Open — parse-time `CompilationError` is wrapped/replaced

`tests/functional/basic/test_invalid_reference.py::test_undefined_value`
is `xfail` under `v2_self`. The test expects parse to raise
`CompilationError`. Under the v2 dispatch:

- Real v2: the parser exits non-zero, `_run_v2` raises
  `V2ParserError` with the captured stderr. Never
  `CompilationError`.
- v2_self shim: the inner `run_dbt(["parse"])` raises
  `CompilationError` inside `_fake_run_v2`, but it propagates
  through `parse_with_v2`'s temp-dir context manager and through
  the outer `run_dbt`'s `expect_pass` enforcement, surfacing as
  `AssertionError`.

This is a real contract change in the v2 flow: parse-time error types
are no longer the same exception classes downstream callers see today.
Two follow-ups, in order:

1. Decide the v2 parse-error contract. Today it's `V2ParserError`
   from the real path and a wrapped/wrong type from the shim. Either
   harden the shim to translate parse errors into `V2ParserError`
   (so v2_self matches the real v2 parser), or change `parse_with_v2` to
   re-raise the original exception class when it can detect one.
2. Update tests asserting on `CompilationError` from parse to assert
   on the agreed v2 contract when running under v2. The
   `xfail_v2_self` helper in `v2_self_parser.py` exists for marking
   these in the meantime — `pytest.xfail`, not `skip`, so a future
   fix flips the test to `XPASS` and surfaces the change.

### Not divergences (notable)

`tests/functional/basic/test_project.py` has 4 simple
`run_dbt(["run"])` ports (version-missing / version-valid variants)
that pass under both modes — pre-parse `dbt_project.yml` shape is
faithfully preserved across the manifest.json handoff. The other 5
tests in that file trigger `ProjectContractError` before parse runs
and were deliberately not ported.

## Layout

```
plugin.py            pytest plugin: CLI flag, marker, parser_mode fixture
v2_self_parser.py    shim + run_dbt_for_mode helpers + xfail_v2_self
README.md            this file
```
