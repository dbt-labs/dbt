from dbt.adapters.events.base_types import WarnLevel, InfoLevel, ErrorLevel, DebugLevel
from dbt.common.ui import line_wrap_message, warning_tag


def format_adapter_message(name, base_msg, args) -> str:
    # only apply formatting if there are arguments to format.
    # avoids issues like "dict: {k: v}".format() which results in `KeyError 'k'`
    msg = base_msg if len(args) == 0 else base_msg.format(*args)
    return f"{name} adapter: {msg}"


# =======================================================
# D - Deprecations
# =======================================================


class CollectFreshnessReturnSignature(WarnLevel):
    def code(self) -> str:
        return "D012"

    def message(self) -> str:
        description = (
            "The 'collect_freshness' macro signature has changed to return the full "
            "query result, rather than just a table of values. See the v1.5 migration guide "
            "for details on how to update your custom macro: https://docs.getdbt.com/guides/migration/versions/upgrading-to-v1.5"
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class AdapterDeprecationWarning(WarnLevel):
    def code(self) -> str:
        return "D005"

    def message(self) -> str:
        description = (
            f"The adapter function `adapter.{self.old_name}` is deprecated and will be removed in "
            f"a future release of dbt. Please use `adapter.{self.new_name}` instead. "
            f"\n\nDocumentation for {self.new_name} can be found here:"
            f"\n\nhttps://docs.getdbt.com/docs/adapter"
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


# =======================================================
# E - DB Adapter
# =======================================================


class AdapterEventDebug(DebugLevel):
    def code(self) -> str:
        return "E001"

    def message(self) -> str:
        return format_adapter_message(self.name, self.base_msg, self.args)


class AdapterEventInfo(InfoLevel):
    def code(self) -> str:
        return "E002"

    def message(self) -> str:
        return format_adapter_message(self.name, self.base_msg, self.args)


class AdapterEventWarning(WarnLevel):
    def code(self) -> str:
        return "E003"

    def message(self) -> str:
        return format_adapter_message(self.name, self.base_msg, self.args)


class AdapterEventError(ErrorLevel):
    def code(self) -> str:
        return "E004"

    def message(self) -> str:
        return format_adapter_message(self.name, self.base_msg, self.args)


class NewConnection(DebugLevel):
    def code(self) -> str:
        return "E005"

    def message(self) -> str:
        return f"Acquiring new {self.conn_type} connection '{self.conn_name}'"


class ConnectionReused(DebugLevel):
    def code(self) -> str:
        return "E006"

    def message(self) -> str:
        return f"Re-using an available connection from the pool (formerly {self.orig_conn_name}, now {self.conn_name})"


class ConnectionLeftOpenInCleanup(DebugLevel):
    def code(self) -> str:
        return "E007"

    def message(self) -> str:
        return f"Connection '{self.conn_name}' was left open."


class ConnectionClosedInCleanup(DebugLevel):
    def code(self) -> str:
        return "E008"

    def message(self) -> str:
        return f"Connection '{self.conn_name}' was properly closed."


class RollbackFailed(DebugLevel):
    def code(self) -> str:
        return "E009"

    def message(self) -> str:
        return f"Failed to rollback '{self.conn_name}'"


class ConnectionClosed(DebugLevel):
    def code(self) -> str:
        return "E010"

    def message(self) -> str:
        return f"On {self.conn_name}: Close"


class ConnectionLeftOpen(DebugLevel):
    def code(self) -> str:
        return "E011"

    def message(self) -> str:
        return f"On {self.conn_name}: No close available on handle"


class Rollback(DebugLevel):
    def code(self) -> str:
        return "E012"

    def message(self) -> str:
        return f"On {self.conn_name}: ROLLBACK"


class CacheMiss(DebugLevel):
    def code(self) -> str:
        return "E013"

    def message(self) -> str:
        return (
            f'On "{self.conn_name}": cache miss for schema '
            f'"{self.database}.{self.schema}", this is inefficient'
        )


class ListRelations(DebugLevel):
    def code(self) -> str:
        return "E014"

    def message(self) -> str:
        identifiers_str = ", ".join(r.identifier for r in self.relations)
        return f"While listing relations in database={self.database}, schema={self.schema}, found: {identifiers_str}"


class ConnectionUsed(DebugLevel):
    def code(self) -> str:
        return "E015"

    def message(self) -> str:
        return f'Using {self.conn_type} connection "{self.conn_name}"'


class SQLQuery(DebugLevel):
    def code(self) -> str:
        return "E016"

    def message(self) -> str:
        return f"On {self.conn_name}: {self.sql}"


class SQLQueryStatus(DebugLevel):
    def code(self) -> str:
        return "E017"

    def message(self) -> str:
        return f"SQL status: {self.status} in {self.elapsed} seconds"


class SQLCommit(DebugLevel):
    def code(self) -> str:
        return "E018"

    def message(self) -> str:
        return f"On {self.conn_name}: COMMIT"


class ColTypeChange(DebugLevel):
    def code(self) -> str:
        return "E019"

    def message(self) -> str:
        return f"Changing col type from {self.orig_type} to {self.new_type} in table {self.table}"


class SchemaCreation(DebugLevel):
    def code(self) -> str:
        return "E020"

    def message(self) -> str:
        return f'Creating schema "{self.relation}"'


class SchemaDrop(DebugLevel):
    def code(self) -> str:
        return "E021"

    def message(self) -> str:
        return f'Dropping schema "{self.relation}".'


class CacheAction(DebugLevel):
    def code(self) -> str:
        return "E022"

    def format_ref_key(self, ref_key) -> str:
        return f"(database={ref_key.database}, schema={ref_key.schema}, identifier={ref_key.identifier})"

    def message(self) -> str:
        ref_key = self.format_ref_key(self.ref_key)
        ref_key_2 = self.format_ref_key(self.ref_key_2)
        ref_key_3 = self.format_ref_key(self.ref_key_3)
        ref_list = []
        for rfk in self.ref_list:
            ref_list.append(self.format_ref_key(rfk))
        if self.action == "add_link":
            return f"adding link, {ref_key} references {ref_key_2}"
        elif self.action == "add_relation":
            return f"adding relation: {ref_key}"
        elif self.action == "drop_missing_relation":
            return f"dropped a nonexistent relationship: {ref_key}"
        elif self.action == "drop_cascade":
            return f"drop {ref_key} is cascading to {ref_list}"
        elif self.action == "drop_relation":
            return f"Dropping relation: {ref_key}"
        elif self.action == "update_reference":
            return (
                f"updated reference from {ref_key} -> {ref_key_3} to "
                f"{ref_key_2} -> {ref_key_3}"
            )
        elif self.action == "temporary_relation":
            return f"old key {ref_key} not found in self.relations, assuming temporary"
        elif self.action == "rename_relation":
            return f"Renaming relation {ref_key} to {ref_key_2}"
        elif self.action == "uncached_relation":
            return (
                f"{ref_key_2} references {ref_key} "
                f"but {self.ref_key.database}.{self.ref_key.schema}"
                "is not in the cache, skipping assumed external relation"
            )
        else:
            return ref_key


# Skipping E023, E024, E025, E026, E027, E028, E029, E030


class CacheDumpGraph(DebugLevel):
    def code(self) -> str:
        return "E031"

    def message(self) -> str:
        return f"dump {self.before_after} {self.action} : {self.dump}"


# Skipping E032, E033
class AdapterImportError(InfoLevel):
    def code(self) -> str:
        return "E035"

    def message(self) -> str:
        return f"Error importing adapter: {self.exc}"


class PluginLoadError(DebugLevel):
    def code(self) -> str:
        return "E036"

    def message(self) -> str:
        return f"{self.exc_info}"


class NewConnectionOpening(DebugLevel):
    def code(self) -> str:
        return "E037"

    def message(self) -> str:
        return f"Opening a new connection, currently in state {self.connection_state}"


class CodeExecution(DebugLevel):
    def code(self) -> str:
        return "E038"

    def message(self) -> str:
        return f"On {self.conn_name}: {self.code_content}"


class CodeExecutionStatus(DebugLevel):
    def code(self) -> str:
        return "E039"

    def message(self) -> str:
        return f"Execution status: {self.status} in {self.elapsed} seconds"


class CatalogGenerationError(WarnLevel):
    def code(self) -> str:
        return "E040"

    def message(self) -> str:
        return f"Encountered an error while generating catalog: {self.exc}"


class WriteCatalogFailure(ErrorLevel):
    def code(self) -> str:
        return "E041"

    def message(self) -> str:
        return (
            f"dbt encountered {self.num_exceptions} failure{(self.num_exceptions != 1) * 's'} "
            "while writing the catalog"
        )


class CatalogWritten(InfoLevel):
    def code(self) -> str:
        return "E042"

    def message(self) -> str:
        return f"Catalog written to {self.path}"


class CannotGenerateDocs(InfoLevel):
    def code(self) -> str:
        return "E043"

    def message(self) -> str:
        return "compile failed, cannot generate docs"


class BuildingCatalog(InfoLevel):
    def code(self) -> str:
        return "E044"

    def message(self) -> str:
        return "Building catalog"


class DatabaseErrorRunningHook(InfoLevel):
    def code(self) -> str:
        return "E045"

    def message(self) -> str:
        return f"Database error while running {self.hook_type}"


class HooksRunning(InfoLevel):
    def code(self) -> str:
        return "E046"

    def message(self) -> str:
        plural = "hook" if self.num_hooks == 1 else "hooks"
        return f"Running {self.num_hooks} {self.hook_type} {plural}"


class FinishedRunningStats(InfoLevel):
    def code(self) -> str:
        return "E047"

    def message(self) -> str:
        return f"Finished running {self.stat_line}{self.execution} ({self.execution_time:0.2f}s)."


class ConstraintNotEnforced(WarnLevel):
    def code(self) -> str:
        return "E048"

    def message(self) -> str:
        msg = (
            f"The constraint type {self.constraint} is not enforced by {self.adapter}. "
            "The constraint will be included in this model's DDL statement, but it will not "
            "guarantee anything about the underlying data. Set 'warn_unenforced: false' on "
            "this constraint to ignore this warning."
        )
        return line_wrap_message(warning_tag(msg))


class ConstraintNotSupported(WarnLevel):
    def code(self) -> str:
        return "E049"

    def message(self) -> str:
        msg = (
            f"The constraint type {self.constraint} is not supported by {self.adapter}, and will "
            "be ignored. Set 'warn_unsupported: false' on this constraint to ignore this warning."
        )
        return line_wrap_message(warning_tag(msg))
