import abc
from concurrent.futures import as_completed, Future
from contextlib import contextmanager
from datetime import datetime
from enum import Enum
import time
from itertools import chain
from typing import (
    Any,
    Callable,
    Dict,
    Iterable,
    Iterator,
    List,
    Mapping,
    Optional,
    Set,
    Tuple,
    Type,
    TypedDict,
    Union,
)

from dbt.adapters.capability import Capability, CapabilityDict
from dbt.contracts.graph.nodes import ColumnLevelConstraint, ConstraintType, ModelLevelConstraint

import agate
import pytz

from dbt.exceptions import (
    DbtInternalError,
    DbtRuntimeError,
    DbtValidationError,
    MacroArgTypeError,
    MacroResultError,
    NotImplementedError,
    NullRelationCacheAttemptedError,
    NullRelationDropAttemptedError,
    QuoteConfigTypeError,
    RelationReturnedMultipleResultsError,
    RenameToNoneAttemptedError,
    SnapshotTargetIncompleteError,
    SnapshotTargetNotSnapshotTableError,
    UnexpectedNonTimestampError,
    UnexpectedNullError,
)

from dbt.adapters.protocol import AdapterConfig
from dbt.clients.agate_helper import (
    empty_table,
    get_column_value_uncased,
    merge_tables,
    table_from_rows,
    Integer,
)
from dbt.clients.jinja import MacroGenerator
from dbt.contracts.graph.manifest import Manifest, MacroManifest
from dbt.contracts.graph.nodes import ResultNode
from dbt.events.functions import fire_event, warn_or_error
from dbt.events.types import (
    CacheMiss,
    ListRelations,
    CodeExecution,
    CodeExecutionStatus,
    CatalogGenerationError,
    ConstraintNotSupported,
    ConstraintNotEnforced,
)
from dbt.utils import filter_null_values, executor, cast_to_str, AttrDict

from dbt.adapters.base.connections import Connection, AdapterResponse, BaseConnectionManager
from dbt.adapters.base.meta import AdapterMeta, available
from dbt.adapters.base.relation import (
    ComponentName,
    BaseRelation,
    InformationSchema,
    SchemaSearchMap,
)
from dbt.adapters.base import Column as BaseColumn
from dbt.adapters.base import Credentials
from dbt.adapters.cache import RelationsCache, _make_ref_key_dict
from dbt import deprecations

GET_CATALOG_MACRO_NAME = "get_catalog"
GET_CATALOG_RELATIONS_MACRO_NAME = "get_catalog_relations"
FRESHNESS_MACRO_NAME = "collect_freshness"
GET_RELATION_LAST_MODIFIED_MACRO_NAME = "get_relation_last_modified"


class ConstraintSupport(str, Enum):
    ENFORCED = "enforced"
    NOT_ENFORCED = "not_enforced"
    NOT_SUPPORTED = "not_supported"


def _expect_row_value(key: str, row: agate.Row):
    if key not in row.keys():
        raise DbtInternalError(
            'Got a row without "{}" column, columns: {}'.format(key, row.keys())
        )
    return row[key]


def _catalog_filter_schemas(manifest: Manifest) -> Callable[[agate.Row], bool]:
    """Return a function that takes a row and decides if the row should be
    included in the catalog output.
    """
    schemas = frozenset((d.lower(), s.lower()) for d, s in manifest.get_used_schemas())

    def test(row: agate.Row) -> bool:
        if "table_database" in row.keys():
            table_database = _expect_row_value("table_database", row)
        else:
            table_database = None

        if "table_schema" in row.keys():
            table_schema = _expect_row_value("table_schema", row)
            # the schema may be present but None, which is not an error and should
            # be filtered out
            if table_schema is None:
                return False
        else:
            table_schema = None

        if table_database and table_schema:
            return (table_database.lower(), table_schema.lower()) in schemas
        elif table_schema:
            return table_schema in {s for _, s in schemas}
        elif table_database:
            return table_database in {d for d, _ in schemas}
        else:
            return False

    return test


def _utc(dt: Optional[datetime], source: Optional[BaseRelation], field_name: str) -> datetime:
    """If dt has a timezone, return a new datetime that's in UTC. Otherwise,
    assume the datetime is already for UTC and add the timezone.
    """
    if dt is None:
        raise UnexpectedNullError(field_name, source)

    elif not hasattr(dt, "tzinfo"):
        raise UnexpectedNonTimestampError(field_name, source, dt)

    elif dt.tzinfo:
        return dt.astimezone(pytz.UTC)
    else:
        return dt.replace(tzinfo=pytz.UTC)


def _relation_name(rel: Optional[BaseRelation]) -> str:
    if rel is None:
        return "null relation"
    else:
        return str(rel)


def log_code_execution(code_execution_function):
    # decorator to log code and execution time
    if code_execution_function.__name__ != "submit_python_job":
        raise ValueError("this should be only used to log submit_python_job now")

    def execution_with_log(*args):
        self = args[0]
        connection_name = self.connections.get_thread_connection().name
        fire_event(CodeExecution(conn_name=connection_name, code_content=args[2]))
        start_time = time.time()
        response = code_execution_function(*args)
        fire_event(
            CodeExecutionStatus(
                status=response._message, elapsed=round((time.time() - start_time), 2)
            )
        )
        return response

    return execution_with_log


class PythonJobHelper:
    def __init__(self, parsed_model: Dict, credential: Credentials) -> None:
        raise NotImplementedError("PythonJobHelper is not implemented yet")

    def submit(self, compiled_code: str) -> Any:
        raise NotImplementedError("PythonJobHelper submit function is not implemented yet")


class FreshnessResponse(TypedDict):
    max_loaded_at: datetime
    snapshotted_at: datetime
    age: float  # age in seconds


class BaseAdapter(metaclass=AdapterMeta):
    """The BaseAdapter provides an abstract base class for adapters.

    Adapters must implement the following methods and macros. Some of the
    methods can be safely overridden as a noop, where it makes sense
    (transactions on databases that don't support them, for instance). Those
    methods are marked with a (passable) in their docstrings. Check docstrings
    for type information, etc.

    To implement a macro, implement "${adapter_type}__${macro_name}" in the
    adapter's internal project.

    To invoke a method in an adapter macro, call it on the 'adapter' Jinja
    object using dot syntax.

    To invoke a method in model code, add the @available decorator atop a method
    declaration. Methods are invoked as macros.

    Methods:
        - exception_handler
        - date_function
        - list_schemas
        - drop_relation
        - truncate_relation
        - rename_relation
        - get_columns_in_relation
        - get_column_schema_from_query
        - expand_column_types
        - list_relations_without_caching
        - is_cancelable
        - create_schema
        - drop_schema
        - quote
        - convert_text_type
        - convert_number_type
        - convert_boolean_type
        - convert_datetime_type
        - convert_date_type
        - convert_time_type
        - standardize_grants_dict

    Macros:
        - get_catalog
    """

    Relation: Type[BaseRelation] = BaseRelation
    Column: Type[BaseColumn] = BaseColumn
    ConnectionManager: Type[BaseConnectionManager]

    # A set of clobber config fields accepted by this adapter
    # for use in materializations
    AdapterSpecificConfigs: Type[AdapterConfig] = AdapterConfig

    CONSTRAINT_SUPPORT = {
        ConstraintType.check: ConstraintSupport.NOT_SUPPORTED,
        ConstraintType.not_null: ConstraintSupport.ENFORCED,
        ConstraintType.unique: ConstraintSupport.NOT_ENFORCED,
        ConstraintType.primary_key: ConstraintSupport.NOT_ENFORCED,
        ConstraintType.foreign_key: ConstraintSupport.ENFORCED,
    }

    # This static member variable can be overriden in concrete adapter
    # implementations to indicate adapter support for optional capabilities.
    _capabilities = CapabilityDict({})

    def __init__(self, config) -> None:
        self.config = config
        self.cache = RelationsCache()
        self.connections = self.ConnectionManager(config)
        self._macro_manifest_lazy: Optional[MacroManifest] = None

    ###
    # Methods that pass through to the connection manager
    ###
    def acquire_connection(self, name=None) -> Connection:
        return self.connections.set_connection_name(name)

    def release_connection(self) -> None:
        self.connections.release()

    def cleanup_connections(self) -> None:
        self.connections.cleanup_all()

    def clear_transaction(self) -> None:
        self.connections.clear_transaction()

    def commit_if_has_connection(self) -> None:
        self.connections.commit_if_has_connection()

    def debug_query(self) -> None:
        self.execute("select 1 as id")

    def nice_connection_name(self) -> str:
        conn = self.connections.get_if_exists()
        if conn is None or conn.name is None:
            return "<None>"
        return conn.name

    @contextmanager
    def connection_named(self, name: str, node: Optional[ResultNode] = None) -> Iterator[None]:
        try:
            if self.connections.query_header is not None:
                self.connections.query_header.set(name, node)
            self.acquire_connection(name)
            yield
        finally:
            self.release_connection()
            if self.connections.query_header is not None:
                self.connections.query_header.reset()

    @contextmanager
    def connection_for(self, node: ResultNode) -> Iterator[None]:
        with self.connection_named(node.unique_id, node):
            yield

    @available.parse(lambda *a, **k: ("", empty_table()))
    def execute(
        self, sql: str, auto_begin: bool = False, fetch: bool = False, limit: Optional[int] = None
    ) -> Tuple[AdapterResponse, agate.Table]:
        """Execute the given SQL. This is a thin wrapper around
        ConnectionManager.execute.

        :param str sql: The sql to execute.
        :param bool auto_begin: If set, and dbt is not currently inside a
            transaction, automatically begin one.
        :param bool fetch: If set, fetch results.
        :param Optional[int] limit: If set, only fetch n number of rows
        :return: A tuple of the query status and results (empty if fetch=False).
        :rtype: Tuple[AdapterResponse, agate.Table]
        """
        return self.connections.execute(sql=sql, auto_begin=auto_begin, fetch=fetch, limit=limit)

    def validate_sql(self, sql: str) -> AdapterResponse:
        """Submit the given SQL to the engine for validation, but not execution.

        This should throw an appropriate exception if the input SQL is invalid, although
        in practice that will generally be handled by delegating to an existing method
        for execution and allowing the error handler to take care of the rest.

        :param str sql: The sql to validate
        """
        raise NotImplementedError("`validate_sql` is not implemented for this adapter!")

    @available.parse(lambda *a, **k: [])
    def get_column_schema_from_query(self, sql: str) -> List[BaseColumn]:
        """Get a list of the Columns with names and data types from the given sql."""
        _, cursor = self.connections.add_select_query(sql)
        columns = [
            self.Column.create(
                column_name, self.connections.data_type_code_to_name(column_type_code)
            )
            # https://peps.python.org/pep-0249/#description
            for column_name, column_type_code, *_ in cursor.description
        ]
        return columns

    @available.parse(lambda *a, **k: ("", empty_table()))
    def get_partitions_metadata(self, table: str) -> Tuple[agate.Table]:
        """
        TODO: Can we move this to dbt-bigquery?
        Obtain partitions metadata for a BigQuery partitioned table.

        :param str table: a partitioned table id, in standard SQL format.
        :return: a partition metadata tuple, as described in
            https://cloud.google.com/bigquery/docs/creating-partitioned-tables#getting_partition_metadata_using_meta_tables.
        :rtype: agate.Table
        """
        if hasattr(self.connections, "get_partitions_metadata"):
            return self.connections.get_partitions_metadata(table=table)
        else:
            raise NotImplementedError(
                "`get_partitions_metadata` is not implemented for this adapter!"
            )

    ###
    # Methods that should never be overridden
    ###
    @classmethod
    def type(cls) -> str:
        """Get the type of this adapter. Types must be class-unique and
        consistent.

        :return: The type name
        :rtype: str
        """
        return cls.ConnectionManager.TYPE

    @property
    def _macro_manifest(self) -> MacroManifest:
        if self._macro_manifest_lazy is None:
            return self.load_macro_manifest()
        return self._macro_manifest_lazy

    def check_macro_manifest(self) -> Optional[MacroManifest]:
        """Return the internal manifest (used for executing macros) if it's
        been initialized, otherwise return None.
        """
        return self._macro_manifest_lazy

    def load_macro_manifest(self, base_macros_only=False) -> MacroManifest:
        # base_macros_only is for the test framework
        if self._macro_manifest_lazy is None:
            # avoid a circular import
            from dbt.parser.manifest import ManifestLoader

            manifest = ManifestLoader.load_macros(
                self.config,
                self.connections.set_query_header,
                base_macros_only=base_macros_only,
            )
            # TODO CT-211
            self._macro_manifest_lazy = manifest  # type: ignore[assignment]
        # TODO CT-211
        return self._macro_manifest_lazy  # type: ignore[return-value]

    def clear_macro_manifest(self):
        if self._macro_manifest_lazy is not None:
            self._macro_manifest_lazy = None

    ###
    # Caching methods
    ###
    def _schema_is_cached(self, database: Optional[str], schema: str) -> bool:
        """Check if the schema is cached, and by default logs if it is not."""

        if (database, schema) not in self.cache:
            fire_event(
                CacheMiss(
                    conn_name=self.nice_connection_name(),
                    database=cast_to_str(database),
                    schema=schema,
                )
            )
            return False
        else:
            return True

    def _get_cache_schemas(self, manifest: Manifest) -> Set[BaseRelation]:
        """Get the set of schema relations that the cache logic needs to
        populate. This means only executable nodes are included.
        """
        # the cache only cares about executable nodes
        return {
            self.Relation.create_from(self.config, node).without_identifier()
            for node in manifest.nodes.values()
            if (node.is_relational and not node.is_ephemeral_model and not node.is_external_node)
        }

    def _get_catalog_schemas(self, manifest: Manifest) -> SchemaSearchMap:
        """Get a mapping of each node's "information_schema" relations to a
        set of all schemas expected in that information_schema.

        There may be keys that are technically duplicates on the database side,
        for example all of '"foo", 'foo', '"FOO"' and 'FOO' could coexist as
        databases, and values could overlap as appropriate. All values are
        lowercase strings.
        """
        info_schema_name_map = SchemaSearchMap()
        relations = self._get_catalog_relations(manifest)
        for relation in relations:
            info_schema_name_map.add(relation)
        # result is a map whose keys are information_schema Relations without
        # identifiers that have appropriate database prefixes, and whose values
        # are sets of lowercase schema names that are valid members of those
        # databases
        return info_schema_name_map

    def _get_catalog_relations_by_info_schema(
        self, relations
    ) -> Dict[InformationSchema, List[BaseRelation]]:
        relations_by_info_schema: Dict[InformationSchema, List[BaseRelation]] = dict()
        for relation in relations:
            info_schema = relation.information_schema_only()
            if info_schema not in relations_by_info_schema:
                relations_by_info_schema[info_schema] = []
            relations_by_info_schema[info_schema].append(relation)

        return relations_by_info_schema

    def _get_catalog_relations(self, manifest: Manifest) -> List[BaseRelation]:

        nodes = chain(
            [
                node
                for node in manifest.nodes.values()
                if (node.is_relational and not node.is_ephemeral_model)
            ],
            manifest.sources.values(),
        )

        relations = [self.Relation.create_from(self.config, n) for n in nodes]
        return relations

    def _relations_cache_for_schemas(
        self, manifest: Manifest, cache_schemas: Optional[Set[BaseRelation]] = None
    ) -> None:
        """Populate the relations cache for the given schemas. Returns an
        iterable of the schemas populated, as strings.
        """
        if not cache_schemas:
            cache_schemas = self._get_cache_schemas(manifest)
        with executor(self.config) as tpe:
            futures: List[Future[List[BaseRelation]]] = []
            for cache_schema in cache_schemas:
                fut = tpe.submit_connected(
                    self,
                    f"list_{cache_schema.database}_{cache_schema.schema}",
                    self.list_relations_without_caching,
                    cache_schema,
                )
                futures.append(fut)

            for future in as_completed(futures):
                # if we can't read the relations we need to just raise anyway,
                # so just call future.result() and let that raise on failure
                for relation in future.result():
                    self.cache.add(relation)

        # it's possible that there were no relations in some schemas. We want
        # to insert the schemas we query into the cache's `.schemas` attribute
        # so we can check it later
        cache_update: Set[Tuple[Optional[str], str]] = set()
        for relation in cache_schemas:
            if relation.schema:
                cache_update.add((relation.database, relation.schema))
        self.cache.update_schemas(cache_update)

    def set_relations_cache(
        self,
        manifest: Manifest,
        clear: bool = False,
        required_schemas: Optional[Set[BaseRelation]] = None,
    ) -> None:
        """Run a query that gets a populated cache of the relations in the
        database and set the cache on this adapter.
        """
        with self.cache.lock:
            if clear:
                self.cache.clear()
            self._relations_cache_for_schemas(manifest, required_schemas)

    @available
    def cache_added(self, relation: Optional[BaseRelation]) -> str:
        """Cache a new relation in dbt. It will show up in `list relations`."""
        if relation is None:
            name = self.nice_connection_name()
            raise NullRelationCacheAttemptedError(name)
        self.cache.add(relation)
        # so jinja doesn't render things
        return ""

    @available
    def cache_dropped(self, relation: Optional[BaseRelation]) -> str:
        """Drop a relation in dbt. It will no longer show up in
        `list relations`, and any bound views will be dropped from the cache
        """
        if relation is None:
            name = self.nice_connection_name()
            raise NullRelationDropAttemptedError(name)
        self.cache.drop(relation)
        return ""

    @available
    def cache_renamed(
        self,
        from_relation: Optional[BaseRelation],
        to_relation: Optional[BaseRelation],
    ) -> str:
        """Rename a relation in dbt. It will show up with a new name in
        `list_relations`, but bound views will remain bound.
        """
        if from_relation is None or to_relation is None:
            name = self.nice_connection_name()
            src_name = _relation_name(from_relation)
            dst_name = _relation_name(to_relation)
            raise RenameToNoneAttemptedError(src_name, dst_name, name)

        self.cache.rename(from_relation, to_relation)
        return ""

    ###
    # Abstract methods for database-specific values, attributes, and types
    ###
    @classmethod
    @abc.abstractmethod
    def date_function(cls) -> str:
        """Get the date function used by this adapter's database."""
        raise NotImplementedError("`date_function` is not implemented for this adapter!")

    @classmethod
    @abc.abstractmethod
    def is_cancelable(cls) -> bool:
        raise NotImplementedError("`is_cancelable` is not implemented for this adapter!")

    ###
    # Abstract methods about schemas
    ###
    @abc.abstractmethod
    def list_schemas(self, database: str) -> List[str]:
        """Get a list of existing schemas in database"""
        raise NotImplementedError("`list_schemas` is not implemented for this adapter!")

    @available.parse(lambda *a, **k: False)
    def check_schema_exists(self, database: str, schema: str) -> bool:
        """Check if a schema exists.

        The default implementation of this is potentially unnecessarily slow,
        and adapters should implement it if there is an optimized path (and
        there probably is)
        """
        search = (s.lower() for s in self.list_schemas(database=database))
        return schema.lower() in search

    ###
    # Abstract methods about relations
    ###
    @abc.abstractmethod
    @available.parse_none
    def drop_relation(self, relation: BaseRelation) -> None:
        """Drop the given relation.

        *Implementors must call self.cache.drop() to preserve cache state!*
        """
        raise NotImplementedError("`drop_relation` is not implemented for this adapter!")

    @abc.abstractmethod
    @available.parse_none
    def truncate_relation(self, relation: BaseRelation) -> None:
        """Truncate the given relation."""
        raise NotImplementedError("`truncate_relation` is not implemented for this adapter!")

    @abc.abstractmethod
    @available.parse_none
    def rename_relation(self, from_relation: BaseRelation, to_relation: BaseRelation) -> None:
        """Rename the relation from from_relation to to_relation.

        Implementors must call self.cache.rename() to preserve cache state.
        """
        raise NotImplementedError("`rename_relation` is not implemented for this adapter!")

    @abc.abstractmethod
    @available.parse_list
    def get_columns_in_relation(self, relation: BaseRelation) -> List[BaseColumn]:
        """Get a list of the columns in the given Relation."""
        raise NotImplementedError("`get_columns_in_relation` is not implemented for this adapter!")

    @available.deprecated("get_columns_in_relation", lambda *a, **k: [])
    def get_columns_in_table(self, schema: str, identifier: str) -> List[BaseColumn]:
        """DEPRECATED: Get a list of the columns in the given table."""
        relation = self.Relation.create(
            database=self.config.credentials.database,
            schema=schema,
            identifier=identifier,
            quote_policy=self.config.quoting,
        )
        return self.get_columns_in_relation(relation)

    @abc.abstractmethod
    def expand_column_types(self, goal: BaseRelation, current: BaseRelation) -> None:
        """Expand the current table's types to match the goal table. (passable)

        :param self.Relation goal: A relation that currently exists in the
            database with columns of the desired types.
        :param self.Relation current: A relation that currently exists in the
            database with columns of unspecified types.
        """
        raise NotImplementedError(
            "`expand_target_column_types` is not implemented for this adapter!"
        )

    @abc.abstractmethod
    def list_relations_without_caching(self, schema_relation: BaseRelation) -> List[BaseRelation]:
        """List relations in the given schema, bypassing the cache.

        This is used as the underlying behavior to fill the cache.

        :param schema_relation: A relation containing the database and schema
            as appropraite for the underlying data warehouse
        :return: The relations in schema
        :rtype: List[self.Relation]
        """
        raise NotImplementedError(
            "`list_relations_without_caching` is not implemented for this adapter!"
        )

    ###
    # Methods about grants
    ###
    @available
    def standardize_grants_dict(self, grants_table: agate.Table) -> dict:
        """Translate the result of `show grants` (or equivalent) to match the
        grants which a user would configure in their project.

        Ideally, the SQL to show grants should also be filtering:
        filter OUT any grants TO the current user/role (e.g. OWNERSHIP).
        If that's not possible in SQL, it can be done in this method instead.

        :param grants_table: An agate table containing the query result of
            the SQL returned by get_show_grant_sql
        :return: A standardized dictionary matching the `grants` config
        :rtype: dict
        """
        grants_dict: Dict[str, List[str]] = {}
        for row in grants_table:
            grantee = row["grantee"]
            privilege = row["privilege_type"]
            if privilege in grants_dict.keys():
                grants_dict[privilege].append(grantee)
            else:
                grants_dict.update({privilege: [grantee]})
        return grants_dict

    ###
    # Provided methods about relations
    ###
    @available.parse_list
    def get_missing_columns(
        self, from_relation: BaseRelation, to_relation: BaseRelation
    ) -> List[BaseColumn]:
        """Returns a list of Columns in from_relation that are missing from
        to_relation.
        """
        if not isinstance(from_relation, self.Relation):
            raise MacroArgTypeError(
                method_name="get_missing_columns",
                arg_name="from_relation",
                got_value=from_relation,
                expected_type=self.Relation,
            )

        if not isinstance(to_relation, self.Relation):
            raise MacroArgTypeError(
                method_name="get_missing_columns",
                arg_name="to_relation",
                got_value=to_relation,
                expected_type=self.Relation,
            )

        from_columns = {col.name: col for col in self.get_columns_in_relation(from_relation)}

        to_columns = {col.name: col for col in self.get_columns_in_relation(to_relation)}

        missing_columns = set(from_columns.keys()) - set(to_columns.keys())

        return [col for (col_name, col) in from_columns.items() if col_name in missing_columns]

    @available.parse_none
    def valid_snapshot_target(self, relation: BaseRelation) -> None:
        """Ensure that the target relation is valid, by making sure it has the
        expected columns.

        :param Relation relation: The relation to check
        :raises InvalidMacroArgType: If the columns are
            incorrect.
        """
        if not isinstance(relation, self.Relation):
            raise MacroArgTypeError(
                method_name="valid_snapshot_target",
                arg_name="relation",
                got_value=relation,
                expected_type=self.Relation,
            )

        columns = self.get_columns_in_relation(relation)
        names = set(c.name.lower() for c in columns)
        expanded_keys = ("scd_id", "valid_from", "valid_to")
        extra = []
        missing = []
        for legacy in expanded_keys:
            desired = "dbt_" + legacy
            if desired not in names:
                missing.append(desired)
                if legacy in names:
                    extra.append(legacy)

        if missing:
            if extra:
                raise SnapshotTargetIncompleteError(extra, missing)
            else:
                raise SnapshotTargetNotSnapshotTableError(missing)

    @available.parse_none
    def expand_target_column_types(
        self, from_relation: BaseRelation, to_relation: BaseRelation
    ) -> None:
        if not isinstance(from_relation, self.Relation):
            raise MacroArgTypeError(
                method_name="expand_target_column_types",
                arg_name="from_relation",
                got_value=from_relation,
                expected_type=self.Relation,
            )

        if not isinstance(to_relation, self.Relation):
            raise MacroArgTypeError(
                method_name="expand_target_column_types",
                arg_name="to_relation",
                got_value=to_relation,
                expected_type=self.Relation,
            )

        self.expand_column_types(from_relation, to_relation)

    def list_relations(self, database: Optional[str], schema: str) -> List[BaseRelation]:
        if self._schema_is_cached(database, schema):
            return self.cache.get_relations(database, schema)

        schema_relation = self.Relation.create(
            database=database,
            schema=schema,
            identifier="",
            quote_policy=self.config.quoting,
        ).without_identifier()

        # we can't build the relations cache because we don't have a
        # manifest so we can't run any operations.
        relations = self.list_relations_without_caching(schema_relation)

        # if the cache is already populated, add this schema in
        # otherwise, skip updating the cache and just ignore
        if self.cache:
            for relation in relations:
                self.cache.add(relation)
            if not relations:
                # it's possible that there were no relations in some schemas. We want
                # to insert the schemas we query into the cache's `.schemas` attribute
                # so we can check it later
                self.cache.update_schemas([(database, schema)])

        fire_event(
            ListRelations(
                database=cast_to_str(database),
                schema=schema,
                relations=[_make_ref_key_dict(x) for x in relations],
            )
        )

        return relations

    def _make_match_kwargs(self, database: str, schema: str, identifier: str) -> Dict[str, str]:
        quoting = self.config.quoting
        if identifier is not None and quoting["identifier"] is False:
            identifier = identifier.lower()

        if schema is not None and quoting["schema"] is False:
            schema = schema.lower()

        if database is not None and quoting["database"] is False:
            database = database.lower()

        return filter_null_values(
            {
                "database": database,
                "identifier": identifier,
                "schema": schema,
            }
        )

    def _make_match(
        self,
        relations_list: List[BaseRelation],
        database: str,
        schema: str,
        identifier: str,
    ) -> List[BaseRelation]:
        matches = []

        search = self._make_match_kwargs(database, schema, identifier)

        for relation in relations_list:
            if relation.matches(**search):
                matches.append(relation)

        return matches

    @available.parse_none
    def get_relation(self, database: str, schema: str, identifier: str) -> Optional[BaseRelation]:
        relations_list = self.list_relations(database, schema)

        matches = self._make_match(relations_list, database, schema, identifier)

        if len(matches) > 1:
            kwargs = {
                "identifier": identifier,
                "schema": schema,
                "database": database,
            }
            raise RelationReturnedMultipleResultsError(kwargs, matches)

        elif matches:
            return matches[0]

        return None

    @available.deprecated("get_relation", lambda *a, **k: False)
    def already_exists(self, schema: str, name: str) -> bool:
        """DEPRECATED: Return if a model already exists in the database"""
        database = self.config.credentials.database
        relation = self.get_relation(database, schema, name)
        return relation is not None

    ###
    # ODBC FUNCTIONS -- these should not need to change for every adapter,
    #                   although some adapters may override them
    ###
    @abc.abstractmethod
    @available.parse_none
    def create_schema(self, relation: BaseRelation):
        """Create the given schema if it does not exist."""
        raise NotImplementedError("`create_schema` is not implemented for this adapter!")

    @abc.abstractmethod
    @available.parse_none
    def drop_schema(self, relation: BaseRelation):
        """Drop the given schema (and everything in it) if it exists."""
        raise NotImplementedError("`drop_schema` is not implemented for this adapter!")

    @available
    @classmethod
    @abc.abstractmethod
    def quote(cls, identifier: str) -> str:
        """Quote the given identifier, as appropriate for the database."""
        raise NotImplementedError("`quote` is not implemented for this adapter!")

    @available
    def quote_as_configured(self, identifier: str, quote_key: str) -> str:
        """Quote or do not quote the given identifer as configured in the
        project config for the quote key.

        The quote key should be one of 'database' (on bigquery, 'profile'),
        'identifier', or 'schema', or it will be treated as if you set `True`.
        """
        try:
            key = ComponentName(quote_key)
        except ValueError:
            return identifier

        default = self.Relation.get_default_quote_policy().get_part(key)
        if self.config.quoting.get(key, default):
            return self.quote(identifier)
        else:
            return identifier

    @available
    def quote_seed_column(self, column: str, quote_config: Optional[bool]) -> str:
        quote_columns: bool = True
        if isinstance(quote_config, bool):
            quote_columns = quote_config
        elif quote_config is None:
            pass
        else:
            raise QuoteConfigTypeError(quote_config)

        if quote_columns:
            return self.quote(column)
        else:
            return column

    ###
    # Conversions: These must be implemented by concrete implementations, for
    # converting agate types into their sql equivalents.
    ###
    @classmethod
    @abc.abstractmethod
    def convert_text_type(cls, agate_table: agate.Table, col_idx: int) -> str:
        """Return the type in the database that best maps to the agate.Text
        type for the given agate table and column index.

        :param agate_table: The table
        :param col_idx: The index into the agate table for the column.
        :return: The name of the type in the database
        """
        raise NotImplementedError("`convert_text_type` is not implemented for this adapter!")

    @classmethod
    @abc.abstractmethod
    def convert_number_type(cls, agate_table: agate.Table, col_idx: int) -> str:
        """Return the type in the database that best maps to the agate.Number
        type for the given agate table and column index.

        :param agate_table: The table
        :param col_idx: The index into the agate table for the column.
        :return: The name of the type in the database
        """
        raise NotImplementedError("`convert_number_type` is not implemented for this adapter!")

    @classmethod
    def convert_integer_type(cls, agate_table: agate.Table, col_idx: int) -> str:
        """Return the type in the database that best maps to the agate.Number
        type for the given agate table and column index.

        :param agate_table: The table
        :param col_idx: The index into the agate table for the column.
        :return: The name of the type in the database
        """
        return "integer"

    @classmethod
    @abc.abstractmethod
    def convert_boolean_type(cls, agate_table: agate.Table, col_idx: int) -> str:
        """Return the type in the database that best maps to the agate.Boolean
        type for the given agate table and column index.

        :param agate_table: The table
        :param col_idx: The index into the agate table for the column.
        :return: The name of the type in the database
        """
        raise NotImplementedError("`convert_boolean_type` is not implemented for this adapter!")

    @classmethod
    @abc.abstractmethod
    def convert_datetime_type(cls, agate_table: agate.Table, col_idx: int) -> str:
        """Return the type in the database that best maps to the agate.DateTime
        type for the given agate table and column index.

        :param agate_table: The table
        :param col_idx: The index into the agate table for the column.
        :return: The name of the type in the database
        """
        raise NotImplementedError("`convert_datetime_type` is not implemented for this adapter!")

    @classmethod
    @abc.abstractmethod
    def convert_date_type(cls, agate_table: agate.Table, col_idx: int) -> str:
        """Return the type in the database that best maps to the agate.Date
        type for the given agate table and column index.

        :param agate_table: The table
        :param col_idx: The index into the agate table for the column.
        :return: The name of the type in the database
        """
        raise NotImplementedError("`convert_date_type` is not implemented for this adapter!")

    @classmethod
    @abc.abstractmethod
    def convert_time_type(cls, agate_table: agate.Table, col_idx: int) -> str:
        """Return the type in the database that best maps to the
        agate.TimeDelta type for the given agate table and column index.

        :param agate_table: The table
        :param col_idx: The index into the agate table for the column.
        :return: The name of the type in the database
        """
        raise NotImplementedError("`convert_time_type` is not implemented for this adapter!")

    @available
    @classmethod
    def convert_type(cls, agate_table: agate.Table, col_idx: int) -> Optional[str]:
        return cls.convert_agate_type(agate_table, col_idx)

    @classmethod
    def convert_agate_type(cls, agate_table: agate.Table, col_idx: int) -> Optional[str]:
        agate_type: Type = agate_table.column_types[col_idx]
        conversions: List[Tuple[Type, Callable[..., str]]] = [
            (Integer, cls.convert_integer_type),
            (agate.Text, cls.convert_text_type),
            (agate.Number, cls.convert_number_type),
            (agate.Boolean, cls.convert_boolean_type),
            (agate.DateTime, cls.convert_datetime_type),
            (agate.Date, cls.convert_date_type),
            (agate.TimeDelta, cls.convert_time_type),
        ]
        for agate_cls, func in conversions:
            if isinstance(agate_type, agate_cls):
                return func(agate_table, col_idx)

        return None

    ###
    # Operations involving the manifest
    ###
    def execute_macro(
        self,
        macro_name: str,
        manifest: Optional[Manifest] = None,
        project: Optional[str] = None,
        context_override: Optional[Dict[str, Any]] = None,
        kwargs: Optional[Dict[str, Any]] = None,
        text_only_columns: Optional[Iterable[str]] = None,
    ) -> AttrDict:
        """Look macro_name up in the manifest and execute its results.

        :param macro_name: The name of the macro to execute.
        :param manifest: The manifest to use for generating the base macro
            execution context. If none is provided, use the internal manifest.
        :param project: The name of the project to search in, or None for the
            first match.
        :param context_override: An optional dict to update() the macro
            execution context.
        :param kwargs: An optional dict of keyword args used to pass to the
            macro.
        """

        if kwargs is None:
            kwargs = {}
        if context_override is None:
            context_override = {}

        if manifest is None:
            # TODO CT-211
            manifest = self._macro_manifest  # type: ignore[assignment]
        # TODO CT-211
        macro = manifest.find_macro_by_name(  # type: ignore[union-attr]
            macro_name, self.config.project_name, project
        )
        if macro is None:
            if project is None:
                package_name = "any package"
            else:
                package_name = 'the "{}" package'.format(project)

            raise DbtRuntimeError(
                'dbt could not find a macro with the name "{}" in {}'.format(
                    macro_name, package_name
                )
            )
        # This causes a reference cycle, as generate_runtime_macro_context()
        # ends up calling get_adapter, so the import has to be here.
        from dbt.context.providers import generate_runtime_macro_context

        macro_context = generate_runtime_macro_context(
            # TODO CT-211
            macro=macro,
            config=self.config,
            manifest=manifest,  # type: ignore[arg-type]
            package_name=project,
        )
        macro_context.update(context_override)

        macro_function = MacroGenerator(macro, macro_context)

        with self.connections.exception_handler(f"macro {macro_name}"):
            result = macro_function(**kwargs)
        return result

    @classmethod
    def _catalog_filter_table(cls, table: agate.Table, manifest: Manifest) -> agate.Table:
        """Filter the table as appropriate for catalog entries. Subclasses can
        override this to change filtering rules on a per-adapter basis.
        """
        # force database + schema to be strings
        table = table_from_rows(
            table.rows,
            table.column_names,
            text_only_columns=["table_database", "table_schema", "table_name"],
        )
        return table.where(_catalog_filter_schemas(manifest))

    def _get_one_catalog(
        self,
        information_schema: InformationSchema,
        schemas: Set[str],
        manifest: Manifest,
    ) -> agate.Table:
        kwargs = {"information_schema": information_schema, "schemas": schemas}
        table = self.execute_macro(
            GET_CATALOG_MACRO_NAME,
            kwargs=kwargs,
            # pass in the full manifest so we get any local project
            # overrides
            manifest=manifest,
        )

        results = self._catalog_filter_table(table, manifest)  # type: ignore[arg-type]
        return results

    def _get_one_catalog_by_relations(
        self,
        information_schema: InformationSchema,
        relations: List[BaseRelation],
        manifest: Manifest,
    ) -> agate.Table:

        kwargs = {
            "information_schema": information_schema,
            "relations": relations,
        }
        table = self.execute_macro(
            GET_CATALOG_RELATIONS_MACRO_NAME,
            kwargs=kwargs,
            # pass in the full manifest, so we get any local project
            # overrides
            manifest=manifest,
        )

        results = self._catalog_filter_table(table, manifest)  # type: ignore[arg-type]
        return results

    def get_filtered_catalog(
        self, manifest: Manifest, relations: Optional[Set[BaseRelation]] = None
    ):
        catalogs: agate.Table
        if (
            relations is None
            or len(relations) > 100
            or not self.supports(Capability.SchemaMetadataByRelations)
        ):
            # Do it the traditional way. We get the full catalog.
            catalogs, exceptions = self.get_catalog(manifest)
        else:
            # Do it the new way. We try to save time by selecting information
            # only for the exact set of relations we are interested in.
            catalogs, exceptions = self.get_catalog_by_relations(manifest, relations)

        if relations and catalogs:
            relation_map = {
                (
                    r.database.casefold() if r.database else None,
                    r.schema.casefold() if r.schema else None,
                    r.identifier.casefold() if r.identifier else None,
                )
                for r in relations
            }

            def in_map(row: agate.Row):
                if "table_database" in row.keys():
                    d = _expect_row_value("table_database", row)
                    d = d.casefold() if d is not None else None
                else:
                    d = None

                if "table_schema" in row.keys():
                    s = _expect_row_value("table_schema", row)
                    s = s.casefold() if s is not None else None
                else:
                    s = None

                i = _expect_row_value("table_name", row)
                i = i.casefold() if i is not None else None

                return (d, s, i) in relation_map

            catalogs = catalogs.where(in_map)

        return catalogs, exceptions

    def row_matches_relation(self, row: agate.Row, relations: Set[BaseRelation]):
        pass

    def get_catalog(self, manifest: Manifest) -> Tuple[agate.Table, List[Exception]]:
        with executor(self.config) as tpe:
            futures: List[Future[agate.Table]] = []
            schema_map: SchemaSearchMap = self._get_catalog_schemas(manifest)
            for info, schemas in schema_map.items():
                if len(schemas) == 0:
                    continue
                name = ".".join([str(info.database), "information_schema"])
                fut = tpe.submit_connected(
                    self, name, self._get_one_catalog, info, schemas, manifest
                )
                futures.append(fut)

        catalogs, exceptions = catch_as_completed(futures)
        return catalogs, exceptions

    def get_catalog_by_relations(
        self, manifest: Manifest, relations: Set[BaseRelation]
    ) -> Tuple[agate.Table, List[Exception]]:
        with executor(self.config) as tpe:
            futures: List[Future[agate.Table]] = []
            relations_by_schema = self._get_catalog_relations_by_info_schema(relations)
            for info_schema in relations_by_schema:
                name = ".".join([str(info_schema.database), "information_schema"])
                relations = set(relations_by_schema[info_schema])
                fut = tpe.submit_connected(
                    self,
                    name,
                    self._get_one_catalog_by_relations,
                    info_schema,
                    relations,
                    manifest,
                )
                futures.append(fut)

            catalogs, exceptions = catch_as_completed(futures)
            return catalogs, exceptions

    def cancel_open_connections(self):
        """Cancel all open connections."""
        return self.connections.cancel_open()

    def calculate_freshness(
        self,
        source: BaseRelation,
        loaded_at_field: str,
        filter: Optional[str],
        manifest: Optional[Manifest] = None,
    ) -> Tuple[Optional[AdapterResponse], FreshnessResponse]:
        """Calculate the freshness of sources in dbt, and return it"""
        kwargs: Dict[str, Any] = {
            "source": source,
            "loaded_at_field": loaded_at_field,
            "filter": filter,
        }

        # run the macro
        # in older versions of dbt-core, the 'collect_freshness' macro returned the table of results directly
        # starting in v1.5, by default, we return both the table and the adapter response (metadata about the query)
        result: Union[
            AttrDict,  # current: contains AdapterResponse + agate.Table
            agate.Table,  # previous: just table
        ]
        result = self.execute_macro(FRESHNESS_MACRO_NAME, kwargs=kwargs, manifest=manifest)
        if isinstance(result, agate.Table):
            deprecations.warn("collect-freshness-return-signature")
            adapter_response = None
            table = result
        else:
            adapter_response, table = result.response, result.table  # type: ignore[attr-defined]
        # now we have a 1-row table of the maximum `loaded_at_field` value and
        # the current time according to the db.
        if len(table) != 1 or len(table[0]) != 2:
            raise MacroResultError(FRESHNESS_MACRO_NAME, table)
        if table[0][0] is None:
            # no records in the table, so really the max_loaded_at was
            # infinitely long ago. Just call it 0:00 January 1 year UTC
            max_loaded_at = datetime(1, 1, 1, 0, 0, 0, tzinfo=pytz.UTC)
        else:
            max_loaded_at = _utc(table[0][0], source, loaded_at_field)

        snapshotted_at = _utc(table[0][1], source, loaded_at_field)
        age = (snapshotted_at - max_loaded_at).total_seconds()
        freshness: FreshnessResponse = {
            "max_loaded_at": max_loaded_at,
            "snapshotted_at": snapshotted_at,
            "age": age,
        }
        return adapter_response, freshness

    def calculate_freshness_from_metadata(
        self,
        source: BaseRelation,
        manifest: Optional[Manifest] = None,
    ) -> Tuple[Optional[AdapterResponse], FreshnessResponse]:
        kwargs: Dict[str, Any] = {
            "information_schema": source.information_schema_only(),
            "relations": [source],
        }
        result = self.execute_macro(
            GET_RELATION_LAST_MODIFIED_MACRO_NAME, kwargs=kwargs, manifest=manifest
        )
        adapter_response, table = result.response, result.table  # type: ignore[attr-defined]

        try:
            row = table[0]
            last_modified_val = get_column_value_uncased("last_modified", row)
            snapshotted_at_val = get_column_value_uncased("snapshotted_at", row)
        except Exception:
            raise MacroResultError(GET_RELATION_LAST_MODIFIED_MACRO_NAME, table)

        if last_modified_val is None:
            # Interpret missing value as "infinitely long ago"
            max_loaded_at = datetime(1, 1, 1, 0, 0, 0, tzinfo=pytz.UTC)
        else:
            max_loaded_at = _utc(last_modified_val, None, "last_modified")

        snapshotted_at = _utc(snapshotted_at_val, None, "snapshotted_at")

        age = (snapshotted_at - max_loaded_at).total_seconds()

        freshness: FreshnessResponse = {
            "max_loaded_at": max_loaded_at,
            "snapshotted_at": snapshotted_at,
            "age": age,
        }

        return adapter_response, freshness

    def pre_model_hook(self, config: Mapping[str, Any]) -> Any:
        """A hook for running some operation before the model materialization
        runs. The hook can assume it has a connection available.

        The only parameter is a configuration dictionary (the same one
        available in the materialization context). It should be considered
        read-only.

        The pre-model hook may return anything as a context, which will be
        passed to the post-model hook.
        """
        pass

    def post_model_hook(self, config: Mapping[str, Any], context: Any) -> None:
        """A hook for running some operation after the model materialization
        runs. The hook can assume it has a connection available.

        The first parameter is a configuration dictionary (the same one
        available in the materialization context). It should be considered
        read-only.

        The second parameter is the value returned by pre_mdoel_hook.
        """
        pass

    def get_compiler(self):
        from dbt.compilation import Compiler

        return Compiler(self.config)

    # Methods used in adapter tests
    def update_column_sql(
        self,
        dst_name: str,
        dst_column: str,
        clause: str,
        where_clause: Optional[str] = None,
    ) -> str:
        clause = f"update {dst_name} set {dst_column} = {clause}"
        if where_clause is not None:
            clause += f" where {where_clause}"
        return clause

    def timestamp_add_sql(self, add_to: str, number: int = 1, interval: str = "hour") -> str:
        # for backwards compatibility, we're compelled to set some sort of
        # default. A lot of searching has lead me to believe that the
        # '+ interval' syntax used in postgres/redshift is relatively common
        # and might even be the SQL standard's intention.
        return f"{add_to} + interval '{number} {interval}'"

    def string_add_sql(
        self,
        add_to: str,
        value: str,
        location="append",
    ) -> str:
        if location == "append":
            return f"{add_to} || '{value}'"
        elif location == "prepend":
            return f"'{value}' || {add_to}"
        else:
            raise DbtRuntimeError(f'Got an unexpected location value of "{location}"')

    def get_rows_different_sql(
        self,
        relation_a: BaseRelation,
        relation_b: BaseRelation,
        column_names: Optional[List[str]] = None,
        except_operator: str = "EXCEPT",
    ) -> str:
        """Generate SQL for a query that returns a single row with a two
        columns: the number of rows that are different between the two
        relations and the number of mismatched rows.
        """
        # This method only really exists for test reasons.
        names: List[str]
        if column_names is None:
            columns = self.get_columns_in_relation(relation_a)
            names = sorted((self.quote(c.name) for c in columns))
        else:
            names = sorted((self.quote(n) for n in column_names))
        columns_csv = ", ".join(names)

        sql = COLUMNS_EQUAL_SQL.format(
            columns=columns_csv,
            relation_a=str(relation_a),
            relation_b=str(relation_b),
            except_op=except_operator,
        )

        return sql

    @property
    def python_submission_helpers(self) -> Dict[str, Type[PythonJobHelper]]:
        raise NotImplementedError("python_submission_helpers is not specified")

    @property
    def default_python_submission_method(self) -> str:
        raise NotImplementedError("default_python_submission_method is not specified")

    @log_code_execution
    def submit_python_job(self, parsed_model: dict, compiled_code: str) -> AdapterResponse:
        submission_method = parsed_model["config"].get(
            "submission_method", self.default_python_submission_method
        )
        if submission_method not in self.python_submission_helpers:
            raise NotImplementedError(
                "Submission method {} is not supported for current adapter".format(
                    submission_method
                )
            )
        job_helper = self.python_submission_helpers[submission_method](
            parsed_model, self.connections.profile.credentials
        )
        submission_result = job_helper.submit(compiled_code)
        # process submission result to generate adapter response
        return self.generate_python_submission_response(submission_result)

    def generate_python_submission_response(self, submission_result: Any) -> AdapterResponse:
        raise NotImplementedError(
            "Your adapter need to implement generate_python_submission_response"
        )

    def valid_incremental_strategies(self):
        """The set of standard builtin strategies which this adapter supports out-of-the-box.
        Not used to validate custom strategies defined by end users.
        """
        return ["append"]

    def builtin_incremental_strategies(self):
        return ["append", "delete+insert", "merge", "insert_overwrite"]

    @available.parse_none
    def get_incremental_strategy_macro(self, model_context, strategy: str):
        # Construct macro_name from strategy name
        if strategy is None:
            strategy = "default"

        # validate strategies for this adapter
        valid_strategies = self.valid_incremental_strategies()
        valid_strategies.append("default")
        builtin_strategies = self.builtin_incremental_strategies()
        if strategy in builtin_strategies and strategy not in valid_strategies:
            raise DbtRuntimeError(
                f"The incremental strategy '{strategy}' is not valid for this adapter"
            )

        strategy = strategy.replace("+", "_")
        macro_name = f"get_incremental_{strategy}_sql"
        # The model_context should have MacroGenerator callable objects for all macros
        if macro_name not in model_context:
            raise DbtRuntimeError(
                'dbt could not find an incremental strategy macro with the name "{}" in {}'.format(
                    macro_name, self.config.project_name
                )
            )

        # This returns a callable macro
        return model_context[macro_name]

    @classmethod
    def _parse_column_constraint(cls, raw_constraint: Dict[str, Any]) -> ColumnLevelConstraint:
        try:
            ColumnLevelConstraint.validate(raw_constraint)
            return ColumnLevelConstraint.from_dict(raw_constraint)
        except Exception:
            raise DbtValidationError(f"Could not parse constraint: {raw_constraint}")

    @classmethod
    def render_column_constraint(cls, constraint: ColumnLevelConstraint) -> Optional[str]:
        """Render the given constraint as DDL text. Should be overriden by adapters which need custom constraint
        rendering."""
        constraint_expression = constraint.expression or ""

        rendered_column_constraint = None
        if constraint.type == ConstraintType.check and constraint_expression:
            rendered_column_constraint = f"check ({constraint_expression})"
        elif constraint.type == ConstraintType.not_null:
            rendered_column_constraint = f"not null {constraint_expression}"
        elif constraint.type == ConstraintType.unique:
            rendered_column_constraint = f"unique {constraint_expression}"
        elif constraint.type == ConstraintType.primary_key:
            rendered_column_constraint = f"primary key {constraint_expression}"
        elif constraint.type == ConstraintType.foreign_key and constraint_expression:
            rendered_column_constraint = f"references {constraint_expression}"
        elif constraint.type == ConstraintType.custom and constraint_expression:
            rendered_column_constraint = constraint_expression

        if rendered_column_constraint:
            rendered_column_constraint = rendered_column_constraint.strip()

        return rendered_column_constraint

    @available
    @classmethod
    def render_raw_columns_constraints(cls, raw_columns: Dict[str, Dict[str, Any]]) -> List:
        rendered_column_constraints = []

        for v in raw_columns.values():
            col_name = cls.quote(v["name"]) if v.get("quote") else v["name"]
            rendered_column_constraint = [f"{col_name} {v['data_type']}"]
            for con in v.get("constraints", None):
                constraint = cls._parse_column_constraint(con)
                c = cls.process_parsed_constraint(constraint, cls.render_column_constraint)
                if c is not None:
                    rendered_column_constraint.append(c)
            rendered_column_constraints.append(" ".join(rendered_column_constraint))

        return rendered_column_constraints

    @classmethod
    def process_parsed_constraint(
        cls, parsed_constraint: Union[ColumnLevelConstraint, ModelLevelConstraint], render_func
    ) -> Optional[str]:
        if (
            parsed_constraint.warn_unsupported
            and cls.CONSTRAINT_SUPPORT[parsed_constraint.type] == ConstraintSupport.NOT_SUPPORTED
        ):
            warn_or_error(
                ConstraintNotSupported(constraint=parsed_constraint.type.value, adapter=cls.type())
            )
        if (
            parsed_constraint.warn_unenforced
            and cls.CONSTRAINT_SUPPORT[parsed_constraint.type] == ConstraintSupport.NOT_ENFORCED
        ):
            warn_or_error(
                ConstraintNotEnforced(constraint=parsed_constraint.type.value, adapter=cls.type())
            )
        if cls.CONSTRAINT_SUPPORT[parsed_constraint.type] != ConstraintSupport.NOT_SUPPORTED:
            return render_func(parsed_constraint)

        return None

    @classmethod
    def _parse_model_constraint(cls, raw_constraint: Dict[str, Any]) -> ModelLevelConstraint:
        try:
            ModelLevelConstraint.validate(raw_constraint)
            c = ModelLevelConstraint.from_dict(raw_constraint)
            return c
        except Exception:
            raise DbtValidationError(f"Could not parse constraint: {raw_constraint}")

    @available
    @classmethod
    def render_raw_model_constraints(cls, raw_constraints: List[Dict[str, Any]]) -> List[str]:
        return [c for c in map(cls.render_raw_model_constraint, raw_constraints) if c is not None]

    @classmethod
    def render_raw_model_constraint(cls, raw_constraint: Dict[str, Any]) -> Optional[str]:
        constraint = cls._parse_model_constraint(raw_constraint)
        return cls.process_parsed_constraint(constraint, cls.render_model_constraint)

    @classmethod
    def render_model_constraint(cls, constraint: ModelLevelConstraint) -> Optional[str]:
        """Render the given constraint as DDL text. Should be overriden by adapters which need custom constraint
        rendering."""
        constraint_prefix = f"constraint {constraint.name} " if constraint.name else ""
        column_list = ", ".join(constraint.columns)
        if constraint.type == ConstraintType.check and constraint.expression:
            return f"{constraint_prefix}check ({constraint.expression})"
        elif constraint.type == ConstraintType.unique:
            constraint_expression = f" {constraint.expression}" if constraint.expression else ""
            return f"{constraint_prefix}unique{constraint_expression} ({column_list})"
        elif constraint.type == ConstraintType.primary_key:
            constraint_expression = f" {constraint.expression}" if constraint.expression else ""
            return f"{constraint_prefix}primary key{constraint_expression} ({column_list})"
        elif constraint.type == ConstraintType.foreign_key and constraint.expression:
            return f"{constraint_prefix}foreign key ({column_list}) references {constraint.expression}"
        elif constraint.type == ConstraintType.custom and constraint.expression:
            return f"{constraint_prefix}{constraint.expression}"
        else:
            return None

    @classmethod
    def capabilities(cls) -> CapabilityDict:
        return cls._capabilities

    @classmethod
    def supports(cls, capability: Capability) -> bool:
        return bool(cls.capabilities()[capability])


COLUMNS_EQUAL_SQL = """
with diff_count as (
    SELECT
        1 as id,
        COUNT(*) as num_missing FROM (
            (SELECT {columns} FROM {relation_a} {except_op}
             SELECT {columns} FROM {relation_b})
             UNION ALL
            (SELECT {columns} FROM {relation_b} {except_op}
             SELECT {columns} FROM {relation_a})
        ) as a
), table_a as (
    SELECT COUNT(*) as num_rows FROM {relation_a}
), table_b as (
    SELECT COUNT(*) as num_rows FROM {relation_b}
), row_count_diff as (
    select
        1 as id,
        table_a.num_rows - table_b.num_rows as difference
    from table_a, table_b
)
select
    row_count_diff.difference as row_count_difference,
    diff_count.num_missing as num_mismatched
from row_count_diff
join diff_count using (id)
""".strip()


def catch_as_completed(
    futures,  # typing: List[Future[agate.Table]]
) -> Tuple[agate.Table, List[Exception]]:
    # catalogs: agate.Table = agate.Table(rows=[])
    tables: List[agate.Table] = []
    exceptions: List[Exception] = []

    for future in as_completed(futures):
        exc = future.exception()
        # we want to re-raise on ctrl+c and BaseException
        if exc is None:
            catalog = future.result()
            tables.append(catalog)
        elif isinstance(exc, KeyboardInterrupt) or not isinstance(exc, Exception):
            raise exc
        else:
            warn_or_error(CatalogGenerationError(exc=str(exc)))
            # exc is not None, derives from Exception, and isn't ctrl+c
            exceptions.append(exc)
    return merge_tables(tables), exceptions
