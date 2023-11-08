import pytest

from dbt.tests.util import run_dbt
from tests.adapter.dbt.tests.adapter.catalog import files


class CatalogRecognizesIncludePolicy:
    """
    This test addresses: https://github.com/dbt-labs/dbt-core/issues/9013

    To implement this, overwrite `macros` with the version of `get_catalog` that is
    specific to your adapter. Remember to remove database, schema, or both to test the flexibility.
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {"my_view.sql": files.MODELS__VIEW}

    @pytest.fixture(scope="class")
    def macros(self):
        return {"default__get_catalog.sql": files.MACROS__GET_CATALOG}

    def test_include_policy_recognized_during_docs_generate(self, project):
        """
        Running successfully is passing
        """
        run_dbt(["run"])
        run_dbt(["docs", "generate"])
