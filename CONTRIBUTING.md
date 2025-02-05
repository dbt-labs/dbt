# Contributing to `dbt-core`

`dbt-core` is open source software. It is what it is today because community members have opened issues, provided feedback, and [contributed to the knowledge loop](https://www.getdbt.com/dbt-labs/values/). Whether you are a seasoned open source contributor or a first-time committer, we welcome and encourage you to contribute code, documentation, ideas, or problem statements to this project.

1. [About this document](#about-this-document)
2. [Getting the code](#getting-the-code)
3. [Setting up an environment](#setting-up-an-environment)
4. [Running dbt-core in development](#running-dbt-core-in-development)
5. [Testing dbt-core](#testing)
6. [Debugging](#debugging)
7. [Adding or modifying a changelog entry](#adding-or-modifying-a-changelog-entry)
8. [Submitting a Pull Request](#submitting-a-pull-request)
9. [Troubleshooting Tips](#troubleshooting-tips)

## About this document

There are many ways to contribute to the ongoing development of `dbt-core`, such as by participating in discussions and issues. We encourage you to first read our higher-level document: ["Expectations for Open Source Contributors"](https://docs.getdbt.com/docs/contributing/oss-expectations).

The rest of this document serves as a more granular guide for contributing code changes to `dbt-core` (this repository). It is not intended as a guide for using `dbt-core`, and some pieces assume a level of familiarity with Python development (virtualenvs, `pip`, etc). Specific code snippets in this guide assume you are using macOS or Linux and are comfortable with the command line.

If you get stuck, we're happy to help! Drop us a line in the `#dbt-core-development` channel in the [dbt Community Slack](https://community.getdbt.com).

### Notes

- **Adapters:** Is your issue or proposed code change related to a specific [database adapter](https://docs.getdbt.com/docs/available-adapters)? If so, please open issues, PRs, and discussions in that adapter's repository instead.
- **CLA:** Please note that anyone contributing code to `dbt-core` must sign the [Contributor License Agreement](https://docs.getdbt.com/docs/contributor-license-agreements). If you are unable to sign the CLA, the `dbt-core` maintainers will unfortunately be unable to merge any of your Pull Requests. We welcome you to participate in discussions, open issues, and comment on existing ones.
- **Branches:** All pull requests from community contributors should target the `main` branch (default). If the change is needed as a patch for a minor version of dbt that has already been released (or is already a release candidate), a maintainer will backport the changes in your PR to the relevant "latest" release branch (`1.0.latest`, `1.1.latest`, ...). If an issue fix applies to a release branch, that fix should be first committed to the development branch and then to the release branch (rarely release-branch fixes may not apply to `main`).
- **Releases**: Before releasing a new minor version of Core, we prepare a series of alphas and release candidates to allow users (especially employees of dbt Labs!) to test the new version in live environments. This is an important quality assurance step, as it exposes the new code to a wide variety of complicated deployments and can surface bugs before official release. Releases are accessible via our [supported installation methods](https://docs.getdbt.com/docs/core/installation-overview#install-dbt-core).

## Getting the code

### Installing git

You will need `git` in order to download and modify the `dbt-core` source code. On macOS, the best way to download git is to just install [Xcode](https://developer.apple.com/support/xcode/).

### External contributors

If you are not a member of the `dbt-labs` GitHub organization, you can contribute to `dbt-core` by forking the `dbt-core` repository. For a detailed overview on forking, check out the [GitHub docs on forking](https://help.github.com/en/articles/fork-a-repo). In short, you will need to:

1. Fork the `dbt-core` repository
2. Clone your fork locally
3. Check out a new branch for your proposed changes
4. Push changes to your fork
5. Open a pull request against `dbt-labs/dbt-core` from your forked repository

### dbt Labs contributors

If you are a member of the `dbt-labs` GitHub organization, you will have push access to the `dbt-core` repo. Rather than forking `dbt-core` to make your changes, just clone the repository, check out a new branch, and push directly to that branch.

## Setting up an environment

There are some tools that will be helpful to you in developing locally. While this is the list relevant for `dbt-core` development, many of these tools are used commonly across open-source python projects.

### Tools

These are the tools used in `dbt-core` development and testing:

- [`tox`](https://tox.readthedocs.io/en/latest/) to manage virtualenvs across python versions. We currently target the latest patch releases for Python 3.8, 3.9, 3.10 and 3.11
- [`pytest`](https://docs.pytest.org/en/latest/) to define, discover, and run tests
- [`flake8`](https://flake8.pycqa.org/en/latest/) for code linting
- [`black`](https://github.com/psf/black) for code formatting
- [`mypy`](https://mypy.readthedocs.io/en/stable/) for static type checking
- [`pre-commit`](https://pre-commit.com) to easily run those checks
- [`changie`](https://changie.dev/) to create changelog entries, without merge conflicts
- [`make`](https://users.cs.duke.edu/~ola/courses/programming/Makefiles/Makefiles.html) to run multiple setup or test steps in combination. Don't worry too much, nobody _really_ understands how `make` works, and our Makefile aims to be super simple.
- [GitHub Actions](https://github.com/features/actions) for automating tests and checks, once a PR is pushed to the `dbt-core` repository

A deep understanding of these tools in not required to effectively contribute to `dbt-core`, but we recommend checking out the attached documentation if you're interested in learning more about each one.

#### Virtual environments

We strongly recommend using virtual environments when developing code in `dbt-core`. We recommend creating this virtualenv
in the root of the `dbt-core` repository. To create a new virtualenv, run:
```sh
python3 -m venv env
source env/bin/activate
```

This will create and activate a new Python virtual environment.

#### Docker and `docker-compose`

Docker and `docker-compose` are both used in testing. Specific instructions for you OS can be found [here](https://docs.docker.com/get-docker/).


#### Postgres (optional)

For testing, and later in the examples in this document, you may want to have `psql` available so you can poke around in the database and see what happened. We recommend that you use [homebrew](https://brew.sh/) for that on macOS, and your package manager on Linux. You can install any version of the postgres client that you'd like. On macOS, with homebrew setup, you can run:

```sh
brew install postgresql
```

## Running `dbt-core` in development

### Installation

First make sure that you set up your `virtualenv` as described in [Setting up an environment](#setting-up-an-environment).  Also ensure you have the latest version of pip installed with `pip install --upgrade pip`. Next, install `dbt-core` (and its dependencies):

```sh
make dev
```
or, alternatively:
```sh
pip install -r dev-requirements.txt -r editable-requirements.txt
pre-commit install
```

When installed in this way, any changes you make to your local copy of the source code will be reflected immediately in your next `dbt` run.

### Running `dbt-core`

With your virtualenv activated, the `dbt` script should point back to the source code you've cloned on your machine. You can verify this by running `which dbt`. This command should show you a path to an executable in your virtualenv.

Configure your [profile](https://docs.getdbt.com/docs/configure-your-profile) as necessary to connect to your target databases. It may be a good idea to add a new profile pointing to a local Postgres instance, or a specific test sandbox within your data warehouse if appropriate. Make sure to create a profile before running integration tests.

## Testing

Once you're able to manually test that your code change is working as expected, it's important to run existing automated tests, as well as adding some new ones. These tests will ensure that:
- Your code changes do not unexpectedly break other established functionality
- Your code changes can handle all known edge cases
- The functionality you're adding will _keep_ working in the future

Although `dbt-core` works with a number of different databases, you won't need to supply credentials for every one of these databases in your test environment. Instead, you can test most `dbt-core` code changes with Python and Postgres.

### Initial setup

Postgres offers the easiest way to test most `dbt-core` functionality today. They are the fastest to run, and the easiest to set up. To run the Postgres integration tests, you'll have to do one extra step of setting up the test database:

```sh
make setup-db
```
or, alternatively:
```sh
docker-compose up -d database
PGHOST=localhost PGUSER=root PGPASSWORD=password PGDATABASE=postgres bash test/setup_db.sh
```

### Test commands

There are a few methods for running tests locally.

#### Makefile

There are multiple targets in the Makefile to run common test suites and code
checks, most notably:

```sh
# Runs unit tests with py38 and code checks in parallel.
make test
# Runs postgres integration tests with py38 in "fail fast" mode.
make integration
```
> These make targets assume you have a local installation of a recent version of [`tox`](https://tox.readthedocs.io/en/latest/) for unit/integration testing and pre-commit for code quality checks,
> unless you use choose a Docker container to run tests. Run `make help` for more info.

> Running functional tests can fail with repeated errors opening files and other resources.
> If this occurs, check directory permissions and the limit on open file descriptors. The latter needs to be at least 8000.

Check out the other targets in the Makefile to see other commonly used test
suites.

#### `pre-commit`
[`pre-commit`](https://pre-commit.com) takes care of running all code-checks for formatting and linting. Run `make dev` to install `pre-commit` in your local environment (we recommend running this command with a python virtual environment active). This command installs several pip executables including black, mypy, and flake8. Once this is done you can use any of the linter-based make targets as well as a git pre-commit hook that will ensure proper formatting and linting.

#### `tox`

[`tox`](https://tox.readthedocs.io/en/latest/) takes care of managing virtualenvs and install dependencies in order to run tests. You can also run tests in parallel, for example, you can run unit tests for Python 3.8, Python 3.9, Python 3.10 and Python 3.11 checks in parallel with `tox -p`. Also, you can run unit tests for specific python versions with `tox -e py38`. The configuration for these tests in located in `tox.ini`.

#### `pytest`

Finally, you can also run a specific test or group of tests using [`pytest`](https://docs.pytest.org/en/latest/) directly. With a virtualenv active and dev dependencies installed you can do things like:

```sh
# run all unit tests in a file
python3 -m pytest tests/unit/test_invocation_id.py
# run a specific unit test
python3 -m pytest tests/unit/test_invocation_id.py::TestInvocationId::test_invocation_id
# run specific Postgres functional tests
python3 -m pytest tests/functional/sources
```

> See [pytest usage docs](https://docs.pytest.org/en/6.2.x/usage.html) for an overview of useful command-line options.

### Unit, Integration, Functional?

Here are some general rules for adding tests:
* unit tests (`tests/unit`) don’t need to access a database; "pure Python" tests should be written as unit tests
* functional tests (`tests/functional`) cover anything that interacts with a database, namely adapter

## Debugging

1. The logs for a `dbt run` have stack traces and other information for debugging errors (in `logs/dbt.log` in your project directory).
2. Try using a debugger, like `ipdb`. For pytest: `--pdb --pdbcls=IPython.terminal.debugger:pdb`
3. Sometimes, it’s easier to debug on a single thread: `dbt --single-threaded run`
4. To make print statements from Jinja macros:  `{{ log(msg, info=true) }}`
5. You can also add `{{ debug() }}` statements, which will drop you into some auto-generated code that the macro wrote.
6. The dbt “artifacts” are written out to the ‘target’ directory of your dbt project. They are in unformatted json, which can be hard to read. Format them with:
> python -m json.tool target/run_results.json > run_results.json

### Assorted development tips
* Append `# type: ignore` to the end of a line if you need to disable `mypy` on that line.
* Sometimes flake8 complains about lines that are actually fine, in which case you can put a comment on the line such as: # noqa or # noqa: ANNN, where ANNN is the error code that flake8 issues.
* To collect output for `CProfile`, run dbt with the `-r` option and the name of an output file, i.e. `dbt -r dbt.cprof run`. If you just want to profile parsing, you can do: `dbt -r dbt.cprof parse`. `pip` install `snakeviz` to view the output. Run `snakeviz dbt.cprof` and output will be rendered in a browser window.

## Adding or modifying a CHANGELOG Entry

We use [changie](https://changie.dev) to generate `CHANGELOG` entries. **Note:** Do not edit the `CHANGELOG.md` directly. Your modifications will be lost.

Follow the steps to [install `changie`](https://changie.dev/guide/installation/) for your system.

Once changie is installed and your PR is created for a new feature, simply run the following command and changie will walk you through the process of creating a changelog entry:

```shell
changie new
```

Commit the file that's created and your changelog entry is complete!

If you are contributing to a feature already in progress, you will modify the changie yaml file in dbt/.changes/unreleased/ related to your change. If you need help finding this file, please ask within the discussion for the pull request!

You don't need to worry about which `dbt-core` version your change will go into. Just create the changelog entry with `changie`, and open your PR against the `main` branch. All merged changes will be included in the next minor version of `dbt-core`. The Core maintainers _may_ choose to "backport" specific changes in order to patch older minor versions. In that case, a maintainer will take care of that backport after merging your PR, before releasing the new version of `dbt-core`.

## Submitting a Pull Request

Code can be merged into the current development branch `main` by opening a pull request. If the proposal looks like it's on the right track, then a `dbt-core` maintainer will triage the PR and label it as `ready_for_review`. From this point, two code reviewers will be assigned with the aim of responding to any updates to the PR within about one week. They may suggest code revision for style or clarity, or request that you add unit or integration test(s). These are good things! We believe that, with a little bit of help, anyone can contribute high-quality code. Once merged, your contribution will be available for the next release of `dbt-core`.

Automated tests run via GitHub Actions. If you're a first-time contributor, all tests (including code checks and unit tests) will require a maintainer to approve. Changes in the `dbt-core` repository trigger integration tests against Postgres. dbt Labs also provides CI environments in which to test changes to other adapters, triggered by PRs in those adapters' repositories, as well as periodic maintenance checks of each adapter in concert with the latest `dbt-core` code changes.

Once all tests are passing and your PR has been approved, a `dbt-core` maintainer will merge your changes into the active development branch. And that's it! Happy developing :tada:

## Troubleshooting Tips

Sometimes, the content license agreement auto-check bot doesn't find a user's entry in its roster. If you need to force a rerun, add `@cla-bot check` in a comment on the pull request.
