import pytest

from tests.adapter.dbt.tests.adapter.catalog.test_include_policy import (
    CatalogRecognizesIncludePolicy,
)
from tests.functional.catalog_tests import files


class TestCatalogRecognizesIncludePolicy(CatalogRecognizesIncludePolicy):
    @pytest.fixture(scope="class")
    def macros(self):
        return {"postgres__get_catalog_relations.sql": files.MACROS__GET_CATALOG_RELATIONS}
