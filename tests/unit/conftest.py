import pytest

from dbt.artifacts.resources import Quoting, SourceConfig
from dbt.artifacts.resources.types import NodeType
from dbt.contracts.graph.nodes import SourceDefinition

# All manifest related fixtures.
from tests.unit.utils.adapter import *  # noqa
from tests.unit.utils.config import *  # noqa
from tests.unit.utils.event_manager import *  # noqa
from tests.unit.utils.flags import *  # noqa
from tests.unit.utils.manifest import *  # noqa
from tests.unit.utils.project import *  # noqa


@pytest.fixture
def basic_parsed_source_definition_object():
    return SourceDefinition(
        columns={},
        database="some_db",
        description="",
        fqn=["test", "source", "my_source", "my_source_table"],
        identifier="my_source_table",
        loader="stitch",
        name="my_source_table",
        original_file_path="/root/models/sources.yml",
        package_name="test",
        path="/root/models/sources.yml",
        quoting=Quoting(),
        resource_type=NodeType.Source,
        schema="some_schema",
        source_description="my source description",
        source_name="my_source",
        unique_id="test.source.my_source.my_source_table",
        tags=[],
        config=SourceConfig(),
        group="",
        access=AccessType.Protected,  # noqa
    )
