# AGENTS.md — AI Coding Agent Guidelines for dbt-core

## Project Overview

dbt-core is the open-source core of [dbt](https://www.getdbt.com/) (data build tool). It transforms data in warehouses by running SQL and Python models, managing dependencies, and producing artifacts. The main Python package lives in `core/` and is built with Hatch/Hatchling.

## Repository Layout

```
core/                  # Main dbt-core Python package (pyproject.toml, hatch.toml)
  dbt/                 # Source code
    artifacts/         # Artifact schemas and versioned resource definitions
    cli/               # CLI entry point (Click-based)
    clients/           # Jinja, YAML, git, registry clients
    config/            # Profile, project, and runtime configuration
    context/           # Jinja context providers
    contracts/         # Project and graph contracts (nodes, manifest)
    deps/              # Dependency resolution
    events/            # Structured event logging
    graph/             # DAG selection and selector methods
    materializations/  # Materialization strategies (e.g. incremental, microbatch)
    parser/            # Manifest, model, source, macro parsing
    task/              # One module per dbt command (run, build, test, compile, etc.)
    tests/             # Test utilities and fixtures (not the test suite)
tests/                 # Test suite
  unit/                # Unit tests — no database required
  functional/          # Functional tests — require Postgres
plugins/               # Local Postgres adapter (for integration tests)
schemas/               # JSON schemas for dbt artifacts
docs/                  # Architecture docs and guides
.changes/              # Changie changelog entries
```

### Related Repositories

dbt-core depends on packages maintained in separate repos:
- **dbt-common** (`dbt-labs/dbt-common`) — shared utilities, `dbtClassMixin`
- **dbt-adapters** (`dbt-labs/dbt-adapters`) — adapter interfaces and Postgres adapter
- **dbt-semantic-interfaces**, **dbt-extractor**, **dbt-protos** — other ecosystem packages

## Build System and Dev Setup

- **Build backend:** Hatchling (`core/pyproject.toml`)
- **Python:** ≥3.11 (CI tests 3.11–3.14)
- **Setup:** `cd core && hatch run setup`
- **Entry point:** `dbt = dbt.cli.main:cli`

## Code Style and Formatting

- **Formatter:** Black (line length 99)
- **Import sorting:** isort (profile `"black"`)
- **Linter:** flake8
- **Type checking:** mypy
- **Pre-commit:** Runs all of the above via `hatch run code-quality`

Import order should follow isort conventions:
1. `__future__`
2. Standard library
3. Third-party
4. dbt-internal (`dbt`, `dbt_common`, `dbt_adapters`, `dbt_extractor`, `dbt_semantic_interfaces`)

## Architecture Documentation

Before investigating parsing bugs or adding new resource types, read the relevant doc in `docs/arch/`:

| Doc | Covers |
|---|---|
| `3_Parsing.md` | Full parse flow, `ManifestLoader`, `SchemaParser`, parser hierarchy |
| `3.1_Partial_Parsing.md` | Partial parse internals, `PartialParsing` class, file diff and change detection |
| `3.2_Deferral.md` | State-based deferral |
| `3.3_Semantic_Models.md` | Semantic model parsing (v1 standalone vs v2 inline), partial parsing edge cases, key files |

These docs describe where things live and how they connect — read them before doing exploratory code search.

## Key Architectural Conventions

### Artifact Resources: Import from `dbt.artifacts.resources`, Not Versioned Paths

**Never** import directly from versioned artifact paths like `dbt.artifacts.resources.v1.model`. Instead, import from `dbt.artifacts.resources`:

```python
# WRONG — will fail pre-commit
from dbt.artifacts.resources.v1.model import Model

# RIGHT
from dbt.artifacts.resources import Model
```

The `dbt.artifacts.resources.__init__` re-exports everything from the current version. A pre-commit hook enforces this outside the `artifacts/` module.

### Data Model Layer

dbt-core uses `dataclasses` with `dbtClassMixin` (from `dbt-common`) for serialization, backed by mashumaro. It does **not** use pydantic for its data model hierarchy.

The node type hierarchy has two layers:

1. **Artifact resources** (`dbt.artifacts.resources`) — serializable data definitions
2. **Contract nodes** (`dbt.contracts.graph.nodes`) — runtime node classes that inherit from artifact resources and add behavior

```
BaseResource → GraphResource → BaseNode → GraphNode → ParsedNode → CompiledNode
                                                                     ├── ModelNode
                                                                     ├── SnapshotNode
                                                                     ├── AnalysisNode
                                                                     ├── SingularTestNode
                                                                     ├── GenericTestNode
                                                                     └── ...
```

Each contract node has a `resource_class()` method returning its corresponding artifact resource type and a `to_resource()` method for conversion.

### Parser Pattern

Parsers follow a class hierarchy: `BaseParser → Parser → ConfiguredParser → SQLParser`. The typical flow:

`parse_file()` → `parse_node()` → `_create_parsetime_node()` → `parse_from_dict()` → `render_update()` → `add_result_node()`

Schema-based parsers (`SchemaParser`, `YamlReader` subclasses) read YAML and apply patches to nodes.

## Testing

### Structure

- **Unit tests** (`tests/unit/`): Pure Python, no database. Use mocks and helpers from `tests/unit/utils/`.
- **Functional tests** (`tests/functional/`): Require Postgres. Use the `project` fixture from `dbt.tests.fixtures.project`.

### Running Tests

```sh
cd core
hatch run unit-tests              # Unit tests only
hatch run integration-tests       # Functional tests (requires Postgres)
hatch run test                    # Unit tests + code quality checks
hatch run code-quality            # Pre-commit hooks on all files
```

Or directly with pytest:

```sh
cd core
hatch run python3 -m pytest ../tests/unit/path/to/test_file.py
hatch run python3 -m pytest ../tests/functional/feature_name
```

### Functional Test Pattern

Functional tests use class-scoped fixtures to define project files and run dbt commands:

```python
class TestMyFeature:
    @pytest.fixture(scope="class")
    def models(self):
        return {"my_model.sql": "select 1 as id"}

    def test_run_succeeds(self, project):
        results = run_dbt(["run"])
        assert len(results) == 1
```

Key utilities: `run_dbt()`, `run_dbt_and_capture()`, `get_manifest()`, `get_artifact()` from `dbt.tests.util`.

Having multiple tests in a functional test class will mean that those tests **will share** the underlying dbt project fixture of the class. This means that if a test modifies the underlying project that will affect other tests in the same class. This leads to flakiness and means it is generally best practice to have one test per functional test class.

### Database Setup for Functional Tests

```sh
cd core && hatch run setup-db
# or manually:
docker-compose up -d database
PGHOST=localhost PGUSER=root PGPASSWORD=password PGDATABASE=postgres bash scripts/setup_db.sh
```

## Contributing Guide

General contributing documetatnion can be found in [CONTRIBUTING.md](CONTRIBUTING.md)

## Changelog

Use [changie](https://changie.dev/) — do **not** edit `CHANGELOG.md` directly (it is generated).

```sh
changie new
```

This creates a YAML entry in `.changes/unreleased/`. Changelog kinds: Breaking Changes, Features, Fixes, Docs, Under the Hood, Dependencies, Security.

## Commit Discipline

Separate distinct types of changes into their own commits. Do not combine unrelated changes in a single commit, even if they touch the same file. This keeps the history reviewable and individually revertable.

The following categories of change should each be their own commit:

- **Tidying** — fixing typos, improving variable names, cleaning up whitespace
- **Abstractions** — extracting duplicated logic into a shared function or module
- **Refactors** — restructuring code for readability, performance, or maintainability (without changing behavior)
- **Bug fixes** — correcting incorrect behavior
- **Features** — adding new functionality
- **Tests** — adding or improving tests for existing behavior (coverage gaps, edge cases, flaky test fixes). Tests that accompany a new feature or bug fix belong in that commit, but standalone test work is its own category.
- **Dependencies** — adding, removing, or upgrading dependencies
- **Configuration** — changes to CI workflows, linter settings, build configs, pre-commit hooks, or other tooling

When a task involves more than one of these, make separate commits in a logical order. For example, if a bug fix requires a refactor first, commit the refactor, then commit the fix. If a feature benefits from tidying nearby code, commit the tidying first, then the feature.

Each commit should make sense in isolation: it should pass tests, not break the build, and have a clear message explaining *what* and *why*.

Finally we require all commits to be signed with GPG keys. You can inspect if a GPG is present via `git config --global --get user.signingkey`. If it is not, please help the user setup a github GPG key.

## Pull Requests

- Target the `main` branch
- Signed commits required (GPG)
- CLA signature required for external contributors
- Add a changie entry unless the work done was limited to adding/changing tests, adding/changing comments, adding/changing github actions/workflows, or adding/changing markdown files not used during operation of the engine
