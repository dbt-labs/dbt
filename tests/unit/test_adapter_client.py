from unittest import mock

import dbt.clients.adapter.client
from dbt.adapters.base.plugin import AdapterPlugin
from dbt.adapters.include.global_project import (
    PROJECT_NAME as GLOBAL_PROJECT_NAME,
)


root_plugin = mock.Mock(
    Plugin=AdapterPlugin(
        adapter=mock.MagicMock(type=lambda: "root"),
        credentials=mock.MagicMock(),
        include_path="/path/to/root/plugin",
        dependencies=["childa", "childb"],
        project_name="root",
    )
)

childa = mock.Mock(
    Plugin=AdapterPlugin(
        adapter=mock.MagicMock(type=lambda: "childa"),
        credentials=mock.MagicMock(),
        include_path="/path/to/childa",
        project_name="pkg_childa",
    )
)

childb = mock.Mock(
    Plugin=AdapterPlugin(
        adapter=mock.MagicMock(type=lambda: "childb"),
        credentials=mock.MagicMock(),
        include_path="/path/to/childb",
        dependencies=["childc"],
        project_name="pkg_childb",
    )
)

childc = mock.Mock(
    Plugin=AdapterPlugin(
        adapter=mock.MagicMock(type=lambda: "childc"),
        credentials=mock.MagicMock(),
        include_path="/path/to/childc",
        project_name="pkg_childc",
    )
)

_mock_modules = {
    "root": root_plugin,
    "childa": childa,
    "childb": childb,
    "childc": childc,
}


def mock_get_adapter_by_name(name: str):
    try:
        return _mock_modules[name]
    except KeyError:
        raise RuntimeError(f"test could not find adapter type {name}!")


def test_no_packages():
    assert dbt.clients.adapter.client.get_adapter_package_names(None) == [GLOBAL_PROJECT_NAME]


def test_one_package():
    with mock.patch(
        "dbt.adapters.load_adapter.get_adapter_by_name", wraps=mock_get_adapter_by_name
    ):
        dbt.clients.adapter.client.load_plugin("childc")
        assert dbt.clients.adapter.client.get_adapter_package_names("childc") == [
            "pkg_childc",
            GLOBAL_PROJECT_NAME,
        ]


def test_simple_child_packages():
    with mock.patch(
        "dbt.adapters.load_adapter.get_adapter_by_name", wraps=mock_get_adapter_by_name
    ):
        dbt.clients.adapter.client.load_plugin("childb")
        assert dbt.clients.adapter.client.get_adapter_package_names("childb") == [
            "pkg_childb",
            "pkg_childc",
            GLOBAL_PROJECT_NAME,
        ]


def test_layered_child_packages():
    with mock.patch(
        "dbt.adapters.load_adapter.get_adapter_by_name", wraps=mock_get_adapter_by_name
    ):
        dbt.clients.adapter.load_plugin("root")
        assert dbt.clients.adapter.client.get_adapter_package_names("root") == [
            "root",
            "pkg_childa",
            "pkg_childb",
            "pkg_childc",
            GLOBAL_PROJECT_NAME,
        ]
