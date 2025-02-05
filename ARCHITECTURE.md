The core function of dbt is SQL compilation and execution. Users create projects of dbt resources (models, tests, seeds, snapshots, ...), defined in SQL and YAML files, and they invoke dbt to create, update, or query associated views and tables. Today, dbt makes heavy use of Jinja2 to enable the templating of SQL, and to construct a DAG (Directed Acyclic Graph) from all of the resources in a project. Users can also extend their projects by installing resources (including Jinja macros) from other projects, called "packages."

## dbt-core

Most of the python code in the repository is within the `core/dbt` directory.
- [`single python files`](core/dbt/README.md): A number of individual files, such as 'compilation.py' and 'exceptions.py'

The main subdirectories of core/dbt:
- [`adapters`](core/dbt/adapters/README.md): Define base classes for behavior that is likely to differ across databases
- [`clients`](core/dbt/clients/README.md): Interface with dependencies (agate, jinja) or across operating systems
- [`config`](core/dbt/config/README.md): Reconcile user-supplied configuration from connection profiles, project files, and Jinja macros
- [`context`](core/dbt/context/README.md): Build and expose dbt-specific Jinja functionality
- [`contracts`](core/dbt/contracts/README.md): Define Python objects (dataclasses) that dbt expects to create and validate
- [`deps`](core/dbt/deps/README.md): Package installation and dependency resolution
- [`events`](core/dbt/events/README.md): Logging events
- [`graph`](core/dbt/graph/README.md): Produce a `networkx` DAG of project resources, and selecting those resources given user-supplied criteria
- [`include`](core/dbt/include/README.md): Set up the starter project scaffold.
- [`parser`](core/dbt/parser/README.md): Read project files, validate, construct python objects
- [`task`](core/dbt/task/README.md): Set forth the actions that dbt can perform when invoked

Legacy tests are found in the 'test' directory:
- [`unit tests`](core/dbt/test/unit/README.md): Unit tests
- [`integration tests`](core/dbt/test/integration/README.md): Integration tests

### Invoking dbt

The "tasks" map to top-level dbt commands. So `dbt run` => task.run.RunTask, etc. Some are more like abstract base classes (GraphRunnableTask, for example) but all the concrete types outside of task should map to tasks. Currently one executes at a time. The tasks kick off their “Runners” and those do execute in parallel. The parallelism is managed via a thread pool, in GraphRunnableTask.

core/dbt/task/docs/index.html
This is the docs website code. It comes from the dbt-docs repository, and is generated when a release is packaged.

## Adapters

dbt uses an adapter-plugin pattern to extend support to different databases, warehouses, query engines, etc. 
Note: dbt-postgres used to exist in dbt-core but is now in [a separate repo](https://github.com/dbt-labs/dbt-adapters/dbt-postgres) 

Each adapter is a mix of python, Jinja2, and SQL. The adapter code also makes heavy use of Jinja2 to wrap modular chunks of SQL functionality, define default implementations, and allow plugins to override it.

Each adapter plugin is a standalone python package that includes:

- `dbt/include/[name]`: A "sub-global" dbt project, of YAML and SQL files, that reimplements Jinja macros to use the adapter's supported SQL syntax
- `dbt/adapters/[name]`: Python modules that inherit, and optionally reimplement, the base adapter classes defined in dbt-core
- `setup.py`

The Postgres adapter code is the most central, and many of its implementations are used as the default defined in the dbt-core global project. The greater the distance of a data technology from Postgres, the more its adapter plugin may need to reimplement.

## Testing dbt

The [`test/`](test/) subdirectory includes unit and integration tests that run as continuous integration checks against open pull requests. Unit tests check mock inputs and outputs of specific python functions. Integration tests perform end-to-end dbt invocations against real adapters (Postgres, Redshift, Snowflake, BigQuery) and assert that the results match expectations. See [the contributing guide](CONTRIBUTING.md) for a step-by-step walkthrough of setting up a local development and testing environment.

## Everything else

- [docker](docker/): All dbt versions are published as Docker images on DockerHub. This subfolder contains the `Dockerfile` (constant) and `requirements.txt` (one for each version).
- [etc](etc/): Images for README
- [scripts](scripts/): Helper scripts for testing, releasing, and producing JSON schemas. These are not included in distributions of dbt, nor are they rigorously tested—they're just handy tools for the dbt maintainers :)
