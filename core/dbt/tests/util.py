from contextvars import ContextVar, copy_context
from io import StringIO
import os
import shutil
import yaml
import json
import warnings
from datetime import timezone, datetime
from typing import Any, Dict, List, Optional
from contextlib import contextmanager
from dbt.adapters.factory import Adapter

from dbt.cli.main import dbtRunner
from dbt.logger import log_manager
from dbt.contracts.graph.manifest import Manifest
from dbt_common.context import _INVOCATION_CONTEXT_VAR, InvocationContext
from dbt_common.events.functions import (
    fire_event,
    capture_stdout_logs,
    stop_capture_stdout_logs,
    reset_metadata_vars,
)
from dbt_common.events.base_types import EventLevel
from dbt_common.events.types import Note
from dbt.adapters.base.relation import BaseRelation

# =============================================================================
# Test utilities
#   run_dbt
#   run_dbt_and_capture
#   get_manifest
#   copy_file
#   rm_file
#   write_file
#   read_file
#   mkdir
#   rm_dir
#   get_artifact
#   update_config_file
#   write_config_file
#   get_unique_ids_in_results
#   check_result_nodes_by_name
#   check_result_nodes_by_unique_id

# SQL related utilities that use the adapter
#   run_sql_with_adapter
#   relation_from_name
#   check_relation_types (table/view)
#   check_relations_equal
#   check_relation_has_expected_schema
#   check_relations_equal_with_relations
#   check_table_does_exist
#   check_table_does_not_exist
#   get_relation_columns
#   update_rows
#      generate_update_clause
#
# Classes for comparing fields in dictionaries
#   AnyFloat
#   AnyInteger
#   AnyString
#   AnyStringWith
# =============================================================================


# 'run_dbt' is used in pytest tests to run dbt commands. It will return
# different objects depending on the command that is executed.
# For a run command (and most other commands) it will return a list
# of results. For the 'docs generate' command it returns a CatalogArtifact.
# The first parameter is a list of dbt command line arguments, such as
#   run_dbt(["run", "--vars", "seed_name: base"])
# If the command is expected to fail, pass in "expect_pass=False"):
#   run_dbt(["test"], expect_pass=False)
def run_dbt(
    args: Optional[List[str]] = None,
    expect_pass: bool = True,
):
    # Ignore logbook warnings
    warnings.filterwarnings("ignore", category=DeprecationWarning, module="logbook")

    # reset global vars
    reset_metadata_vars()

    # The logger will complain about already being initialized if
    # we don't do this.
    log_manager.reset_handlers()
    if args is None:
        args = ["run"]

    print("\n\nInvoking dbt with {}".format(args))
    from dbt.flags import get_flags

    flags = get_flags()
    project_dir = getattr(flags, "PROJECT_DIR", None)
    profiles_dir = getattr(flags, "PROFILES_DIR", None)
    if project_dir and "--project-dir" not in args:
        args.extend(["--project-dir", project_dir])
    if profiles_dir and "--profiles-dir" not in args:
        args.extend(["--profiles-dir", profiles_dir])

    dbt = dbtRunner()
    res = dbt.invoke(args)

    # the exception is immediately raised to be caught in tests
    # using a pattern like `with pytest.raises(SomeException):`
    if res.exception is not None:
        raise res.exception

    if expect_pass is not None:
        assert res.success == expect_pass, "dbt exit state did not match expected"

    return res.result


# Use this if you need to capture the command logs in a test.
# If you want the logs that are normally written to a file, you must
# start with the "--debug" flag. The structured schema log CI test
# will turn the logs into json, so you have to be prepared for that.
def run_dbt_and_capture(
    args: Optional[List[str]] = None,
    expect_pass: bool = True,
):
    try:
        stringbuf = StringIO()
        capture_stdout_logs(stringbuf)
        res = run_dbt(args, expect_pass=expect_pass)
        stdout = stringbuf.getvalue()

    finally:
        stop_capture_stdout_logs()

    return res, stdout


def get_logging_events(log_output, event_name):
    logging_events = []
    for log_line in log_output.split("\n"):
        # skip empty lines
        if len(log_line) == 0:
            continue
        # The adapter logging also shows up, so skip non-json lines
        if not log_line.startswith("{"):
            continue
        if event_name in log_line:
            log_dct = json.loads(log_line)
            if log_dct["info"]["name"] == event_name:
                logging_events.append(log_dct)
    return logging_events


# Used in test cases to get the manifest from the partial parsing file
# Note: this uses an internal version of the manifest, and in the future
# parts of it will not be supported for external use.
def get_manifest(project_root) -> Optional[Manifest]:
    path = os.path.join(project_root, "target", "partial_parse.msgpack")
    if os.path.exists(path):
        with open(path, "rb") as fp:
            manifest_mp = fp.read()
        manifest: Manifest = Manifest.from_msgpack(manifest_mp)
        return manifest
    else:
        return None


# Used in test cases to get the run_results.json file.
def get_run_results(project_root) -> Any:
    path = os.path.join(project_root, "target", "run_results.json")
    if os.path.exists(path):
        with open(path) as run_result_text:
            return json.load(run_result_text)
    else:
        return None


# Used in tests to copy a file, usually from a data directory to the project directory
def copy_file(src_path, src, dest_path, dest) -> None:
    # dest is a list, so that we can provide nested directories, like 'models' etc.
    # copy files from the data_dir to appropriate project directory
    shutil.copyfile(
        os.path.join(src_path, src),
        os.path.join(dest_path, *dest),
    )


# Used in tests when you want to remove a file from the project directory
def rm_file(*paths) -> None:
    # remove files from proj_path
    os.remove(os.path.join(*paths))


# Used in tests to write out the string contents of a file to a
# file in the project directory.
# We need to explicitly use encoding="utf-8" because otherwise on
# Windows we'll get codepage 1252 and things might break
def write_file(contents, *paths):
    with open(os.path.join(*paths), "w", encoding="utf-8") as fp:
        fp.write(contents)


def file_exists(*paths):
    """Check if file exists at path"""
    return os.path.exists(os.path.join(*paths))


# Used in test utilities
def read_file(*paths):
    contents = ""
    with open(os.path.join(*paths), "r") as fp:
        contents = fp.read()
    return contents


# To create a directory
def mkdir(directory_path):
    try:
        os.makedirs(directory_path)
    except FileExistsError:
        raise FileExistsError(f"{directory_path} already exists.")


# To remove a directory
def rm_dir(directory_path):
    try:
        shutil.rmtree(directory_path)
    except FileNotFoundError:
        raise FileNotFoundError(f"{directory_path} does not exist.")


def rename_dir(src_directory_path, dest_directory_path):
    os.rename(src_directory_path, dest_directory_path)


# Get an artifact (usually from the target directory) such as
# manifest.json or catalog.json to use in a test
def get_artifact(*paths):
    contents = read_file(*paths)
    dct = json.loads(contents)
    return dct


def write_artifact(dct, *paths):
    json_output = json.dumps(dct)
    write_file(json_output, *paths)


# For updating yaml config files
def update_config_file(updates, *paths):
    current_yaml = read_file(*paths)
    config = yaml.safe_load(current_yaml)
    config.update(updates)
    new_yaml = yaml.safe_dump(config)
    write_file(new_yaml, *paths)


# Write new config file
def write_config_file(data, *paths):
    if type(data) is dict:
        data = yaml.safe_dump(data)
    write_file(data, *paths)


# Get the unique_ids in dbt command results
def get_unique_ids_in_results(results):
    unique_ids = []
    for result in results:
        unique_ids.append(result.node.unique_id)
    return unique_ids


# Check the nodes in the results returned by a dbt run command
def check_result_nodes_by_name(results, names):
    result_names = []
    for result in results:
        result_names.append(result.node.name)
    assert set(names) == set(result_names)


# Check the nodes in the results returned by a dbt run command
def check_result_nodes_by_unique_id(results, unique_ids):
    result_unique_ids = []
    for result in results:
        result_unique_ids.append(result.node.unique_id)
    assert set(unique_ids) == set(result_unique_ids)


# Check datetime is between start and end/now
def check_datetime_between(timestr, start, end=None):
    datefmt = "%Y-%m-%dT%H:%M:%S.%fZ"
    if end is None:
        end = datetime.now(timezone.utc).replace(tzinfo=None)
    parsed = datetime.strptime(timestr, datefmt)
    assert start <= parsed
    assert end >= parsed


class TestProcessingException(Exception):
    pass


# Testing utilities that use adapter code

# Uses:
#    adapter.config.credentials
#    adapter.quote
#    adapter.run_sql_for_tests
def run_sql_with_adapter(adapter, sql, fetch=None):
    if sql.strip() == "":
        return

    # substitute schema and database in sql
    kwargs = {
        "schema": adapter.config.credentials.schema,
        "database": adapter.quote(adapter.config.credentials.database),
    }
    sql = sql.format(**kwargs)

    msg = f'test connection "__test" executing: {sql}'
    fire_event(Note(msg=msg), level=EventLevel.DEBUG)
    with get_connection(adapter) as conn:
        return adapter.run_sql_for_tests(sql, fetch, conn)


# Get a Relation object from the identifier (name of table/view).
# Uses the default database and schema. If you need a relation
# with a different schema, it should be constructed in the test.
# Uses:
#    adapter.Relation
#    adapter.config.credentials
#    Relation.get_default_quote_policy
#    Relation.get_default_include_policy
def relation_from_name(adapter, name: str):
    """reverse-engineer a relation from a given name and
    the adapter. The relation name is split by the '.' character.
    """

    # Different adapters have different Relation classes
    cls = adapter.Relation
    credentials = adapter.config.credentials
    quote_policy = cls.get_default_quote_policy().to_dict()
    include_policy = cls.get_default_include_policy().to_dict()

    # Make sure we have database/schema/identifier parts, even if
    # only identifier was supplied.
    relation_parts = name.split(".")
    if len(relation_parts) == 1:
        relation_parts.insert(0, credentials.schema)
    if len(relation_parts) == 2:
        relation_parts.insert(0, credentials.database)
    kwargs = {
        "database": relation_parts[0],
        "schema": relation_parts[1],
        "identifier": relation_parts[2],
    }

    relation = cls.create(
        include_policy=include_policy,
        quote_policy=quote_policy,
        **kwargs,
    )
    return relation


# Ensure that models with different materialiations have the
# current table/view.
# Uses:
#   adapter.list_relations_without_caching
def check_relation_types(adapter, relation_to_type):
    """
    Relation name to table/view
    {
        "base": "table",
        "other": "view",
    }
    """

    expected_relation_values = {}
    found_relations = []
    schemas = set()

    for key, value in relation_to_type.items():
        relation = relation_from_name(adapter, key)
        expected_relation_values[relation] = value
        schemas.add(relation.without_identifier())

        with get_connection(adapter):
            for schema in schemas:
                found_relations.extend(adapter.list_relations_without_caching(schema))

    for key, value in relation_to_type.items():
        for relation in found_relations:
            # this might be too broad
            if relation.identifier == key:
                assert relation.type == value, (
                    f"Got an unexpected relation type of {relation.type} "
                    f"for relation {key}, expected {value}"
                )


# Replaces assertTablesEqual. assertManyTablesEqual can be replaced
# by doing a separate call for each set of tables/relations.
# Wraps check_relations_equal_with_relations by creating relations
# from the list of names passed in.
def check_relations_equal(adapter, relation_names: List, compare_snapshot_cols=False):
    if len(relation_names) < 2:
        raise TestProcessingException(
            "Not enough relations to compare",
        )
    relations = [relation_from_name(adapter, name) for name in relation_names]
    return check_relations_equal_with_relations(
        adapter, relations, compare_snapshot_cols=compare_snapshot_cols
    )


# Used to check that a particular relation has an expected schema
# expected_schema should look like {"column_name": "expected datatype"}
def check_relation_has_expected_schema(adapter, relation_name, expected_schema: Dict):
    relation = relation_from_name(adapter, relation_name)
    with get_connection(adapter):
        actual_columns = {c.name: c.data_type for c in adapter.get_columns_in_relation(relation)}
    assert (
        actual_columns == expected_schema
    ), f"Actual schema did not match expected, actual: {json.dumps(actual_columns)}"


# This can be used when checking relations in different schemas, by supplying
# a list of relations. Called by 'check_relations_equal'.
# Uses:
#    adapter.get_columns_in_relation
#    adapter.get_rows_different_sql
#    adapter.execute
def check_relations_equal_with_relations(
    adapter: Adapter, relations: List, compare_snapshot_cols=False
):
    with get_connection(adapter):
        basis, compares = relations[0], relations[1:]
        # Skip columns starting with "dbt_" because we don't want to
        # compare those, since they are time sensitive
        # (unless comparing "dbt_" snapshot columns is explicitly enabled)
        column_names = [
            c.name
            for c in adapter.get_columns_in_relation(basis)  # type: ignore
            if not c.name.lower().startswith("dbt_") or compare_snapshot_cols
        ]

        for relation in compares:
            sql = adapter.get_rows_different_sql(basis, relation, column_names=column_names)  # type: ignore
            _, tbl = adapter.execute(sql, fetch=True)
            num_rows = len(tbl)
            assert (
                num_rows == 1
            ), f"Invalid sql query from get_rows_different_sql: incorrect number of rows ({num_rows})"
            num_cols = len(tbl[0])
            assert (
                num_cols == 2
            ), f"Invalid sql query from get_rows_different_sql: incorrect number of cols ({num_cols})"
            row_count_difference = tbl[0][0]
            assert (
                row_count_difference == 0
            ), f"Got {row_count_difference} difference in row count betwen {basis} and {relation}"
            rows_mismatched = tbl[0][1]
            assert (
                rows_mismatched == 0
            ), f"Got {rows_mismatched} different rows between {basis} and {relation}"


# Uses:
#    adapter.update_column_sql
#    adapter.execute
#    adapter.commit_if_has_connection
def update_rows(adapter, update_rows_config):
    """
    {
      "name": "base",
      "dst_col": "some_date"
      "clause": {
          "type": "add_timestamp",
          "src_col": "some_date",
       "where" "id > 10"
    }
    """
    for key in ["name", "dst_col", "clause"]:
        if key not in update_rows_config:
            raise TestProcessingException(f"Invalid update_rows: no {key}")

    clause = update_rows_config["clause"]
    clause = generate_update_clause(adapter, clause)

    where = None
    if "where" in update_rows_config:
        where = update_rows_config["where"]

    name = update_rows_config["name"]
    dst_col = update_rows_config["dst_col"]
    relation = relation_from_name(adapter, name)

    with get_connection(adapter):
        sql = adapter.update_column_sql(
            dst_name=str(relation),
            dst_column=dst_col,
            clause=clause,
            where_clause=where,
        )
        adapter.execute(sql, auto_begin=True)
        adapter.commit_if_has_connection()


# This is called by the 'update_rows' function.
# Uses:
#    adapter.timestamp_add_sql
#    adapter.string_add_sql
def generate_update_clause(adapter, clause) -> str:
    """
    Called by update_rows function. Expects the "clause" dictionary
    documented in 'update_rows.
    """

    if "type" not in clause or clause["type"] not in ["add_timestamp", "add_string"]:
        raise TestProcessingException("invalid update_rows clause: type missing or incorrect")
    clause_type = clause["type"]

    if clause_type == "add_timestamp":
        if "src_col" not in clause:
            raise TestProcessingException("Invalid update_rows clause: no src_col")
        add_to = clause["src_col"]
        kwargs = {k: v for k, v in clause.items() if k in ("interval", "number")}
        with get_connection(adapter):
            return adapter.timestamp_add_sql(add_to=add_to, **kwargs)
    elif clause_type == "add_string":
        for key in ["src_col", "value"]:
            if key not in clause:
                raise TestProcessingException(f"Invalid update_rows clause: no {key}")
        src_col = clause["src_col"]
        value = clause["value"]
        location = clause.get("location", "append")
        with get_connection(adapter):
            return adapter.string_add_sql(src_col, value, location)
    return ""


@contextmanager
def get_connection(adapter, name="_test"):
    with adapter.connection_named(name):
        conn = adapter.connections.get_thread_connection()
        yield conn


# Uses:
#    adapter.get_columns_in_relation
def get_relation_columns(adapter, name):
    relation = relation_from_name(adapter, name)
    with get_connection(adapter):
        columns = adapter.get_columns_in_relation(relation)
        return sorted(((c.name, c.dtype, c.char_size) for c in columns), key=lambda x: x[0])


def check_table_does_not_exist(adapter, name):
    columns = get_relation_columns(adapter, name)
    assert len(columns) == 0


def check_table_does_exist(adapter, name):
    columns = get_relation_columns(adapter, name)
    assert len(columns) > 0


# Utility classes for enabling comparison of dictionaries


class AnyFloat:
    """Any float. Use this in assert calls"""

    def __eq__(self, other):
        return isinstance(other, float)


class AnyInteger:
    """Any Integer. Use this in assert calls"""

    def __eq__(self, other):
        return isinstance(other, int)


class AnyString:
    """Any string. Use this in assert calls"""

    def __eq__(self, other):
        return isinstance(other, str)


class AnyStringWith:
    """AnyStringWith("AUTO")"""

    def __init__(self, contains=None):
        self.contains = contains

    def __eq__(self, other):
        if not isinstance(other, str):
            return False

        if self.contains is None:
            return True

        return self.contains in other

    def __repr__(self):
        return "AnyStringWith<{!r}>".format(self.contains)


def assert_message_in_logs(message: str, logs: str, expected_pass: bool = True):
    # if the logs are json strings, then 'jsonify' the message because of things like escape quotes
    if os.environ.get("DBT_LOG_FORMAT", "") == "json":
        message = message.replace(r'"', r"\"")

    if expected_pass:
        assert message in logs
    else:
        assert message not in logs


def get_project_config(project):
    file_yaml = read_file(project.project_root, "dbt_project.yml")
    return yaml.safe_load(file_yaml)


def set_project_config(project, config):
    config_yaml = yaml.safe_dump(config)
    write_file(config_yaml, project.project_root, "dbt_project.yml")


def get_model_file(project, relation: BaseRelation) -> str:
    return read_file(project.project_root, "models", f"{relation.name}.sql")


def set_model_file(project, relation: BaseRelation, model_sql: str):
    write_file(model_sql, project.project_root, "models", f"{relation.name}.sql")


def safe_set_invocation_context():
    """In order to deal with a problem with the way the pytest runner interacts
    with ContextVars, this function provides a mechanism for setting the
    invocation context reliably, using its name rather than the reference
    variable, which may have been loaded in a separate context."""
    invocation_var: Optional[ContextVar] = next(
        iter([cv for cv in copy_context() if cv.name == _INVOCATION_CONTEXT_VAR.name]), None
    )
    if invocation_var is None:
        invocation_var = _INVOCATION_CONTEXT_VAR
    invocation_var.set(InvocationContext(os.environ))
