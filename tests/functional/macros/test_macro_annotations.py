import pytest

from dbt.events.types import InvalidMacroAnnotation
from dbt.tests.util import run_dbt
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.ui import warning_tag

macros_sql = """
{% macro my_macro(my_arg_1, my_arg_2, my_arg_3) %}
{% endmacro %}
"""

bad_arg_names_macros_yml = """
macros:
  - name: my_macro
    description: This is the macro description.
    arguments:
      - name: my_arg_1
      - name: my_misnamed_arg_2
      - name: my_misnamed_arg_3
"""

bad_arg_count_macros_yml = """
macros:
  - name: my_macro
    arguments:
      - name: my_arg_1
        type: string
        description: This is an argument description.
"""

bad_arg_types_macros_yml = """
macros:
  - name: my_macro
    arguments:
      - name: my_arg_1
        type: string
      - name: my_arg_2
        type: invalid_type
      - name: my_arg_3
        type: int[int]
"""

boolean_types_macros_yml = """
macros:
  - name: my_macro
    arguments:
      - name: my_arg_1
        type: boolean
      - name: my_arg_2
        type: optional[boolean]
      - name: my_arg_3
        type: list[boolean]
"""


bad_everything_types_macros_yml = """
macros:
  - name: my_macro
    arguments:
      - name: my_arg_1
        type: string
      - name: my_wrong_arg_2
        type: invalid_type
"""


class TestMacroDefaultArgMetadata:
    """Test that when the validate_macro_args behavior flag is enabled, macro
    argument names are included in the manifest even if there is no yml patch."""

    @pytest.fixture(scope="class")
    def macros(self):
        return {"macros.sql": macros_sql}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"validate_macro_args": True}}

    def test_macro_default_arg_metadata(self, project) -> None:
        manifest = run_dbt(["parse"])
        my_macro_args = manifest.macros["macro.test.my_macro"].arguments
        assert my_macro_args[0].name == "my_arg_1"
        assert my_macro_args[1].name == "my_arg_2"
        assert my_macro_args[2].name == "my_arg_3"


class TestMacroNameWarnings:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"macros.sql": macros_sql, "macros.yml": bad_arg_names_macros_yml}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"validate_macro_args": True}}

    def test_macro_name_enforcement(self, project) -> None:
        event_catcher = EventCatcher(event_to_catch=InvalidMacroAnnotation)
        run_dbt(["parse"], callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 2
        msg = "Argument my_misnamed_arg_2 in yaml for macro my_macro does not match the jinja"
        assert any(
            [e for e in event_catcher.caught_events if e.info.msg.startswith(warning_tag(msg))]
        )
        msg = "Argument my_misnamed_arg_3 in yaml for macro my_macro does not match the jinja"
        assert any(
            [e for e in event_catcher.caught_events if e.info.msg.startswith(warning_tag(msg))]
        )


class TestMacroTypeWarnings:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"macros.sql": macros_sql, "macros.yml": bad_arg_types_macros_yml}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"validate_macro_args": True}}

    def test_macro_type_warnings(self, project) -> None:
        event_catcher = EventCatcher(event_to_catch=InvalidMacroAnnotation)
        run_dbt(["parse"], callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 2
        msg = "Argument my_arg_2 in the yaml for macro my_macro has an invalid type"
        assert any(
            [e for e in event_catcher.caught_events if e.info.msg.startswith(warning_tag(msg))]
        )
        msg = "Argument my_arg_3 in the yaml for macro my_macro has an invalid type"
        assert any(
            [e for e in event_catcher.caught_events if e.info.msg.startswith(warning_tag(msg))]
        )


class TestMacroBooleanType:
    """Test that `boolean` is accepted as an alias for `bool`, as it is in Fusion."""

    @pytest.fixture(scope="class")
    def macros(self):
        return {"macros.sql": macros_sql, "macros.yml": boolean_types_macros_yml}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"validate_macro_args": True}}

    def test_macro_boolean_type(self, project) -> None:
        event_catcher = EventCatcher(event_to_catch=InvalidMacroAnnotation)
        run_dbt(["parse"], callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 0


class TestMacroNonEnforcement:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"macros.yml": bad_everything_types_macros_yml, "macros.sql": macros_sql}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"validate_macro_args": False}}

    def test_macro_non_enforcement(self, project) -> None:
        event_catcher = EventCatcher(event_to_catch=InvalidMacroAnnotation)
        run_dbt(["parse"], callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 0
