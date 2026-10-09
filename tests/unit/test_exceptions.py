from types import SimpleNamespace

from dbt.exceptions import EnvVarMissingError
from dbt.tests.util import safe_set_invocation_context


class TestEnvVarMissingError:
    """Regression tests for #10803: a missing env var error should say *where*
    the env var was required."""

    def test_message_without_context_is_unchanged(self):
        # Node-less call with no source keeps the original, backwards-compatible message.
        exc = EnvVarMissingError("DBT_MISSING")
        assert exc.get_message() == "Env var required but not provided: 'DBT_MISSING'"

    def test_source_is_included_for_node_less_contexts(self):
        # profiles.yml / dbt_project.yml contexts have no node, so they pass `source`.
        exc = EnvVarMissingError("DBT_DATABASE", source="profiles.yml or packages.yml")
        message = exc.get_message()
        assert "Env var required but not provided: 'DBT_DATABASE'" in message
        assert "(required in profiles.yml or packages.yml)" in message

    def test_node_location_is_included_for_model_contexts(self):
        # Model contexts pass `node`; the base error appends the file location.
        safe_set_invocation_context()
        node = SimpleNamespace(
            resource_type="model",
            name="my_model",
            original_file_path="models/my_model.sql",
        )
        exc = EnvVarMissingError("DBT_MISSING", node=node)
        rendered = str(exc)
        assert "DBT_MISSING" in rendered
        assert "models/my_model.sql" in rendered
