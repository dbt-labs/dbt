import pytest

from dbt.cli.main import dbtRunner

deprecated_model__yml = """
version: 2

models:
  - name: my_model
    description: deprecated
    deprecation_date: 1999-01-01
"""

deprecating_model__yml = """
version: 2

models:
  - name: my_model
    description: deprecating in the future
    deprecation_date: 2999-01-01
"""

model__sql = """
select 1 as Id
"""

dependant_model__sql = """
select * from {{ ref("my_model") }}
"""


class TestModelDeprecationWarning:
    @pytest.fixture(scope="class")
    def models(self):
        return {"my_model.sql": model__sql, "my_schema.yml": deprecated_model__yml}

    def test_deprecation_warning(self, project):
        events = []
        dbtRunner(callbacks=[events.append]).invoke(["parse"])
        matches = list([e for e in events if e.info.name == "DeprecatedModel"])
        assert len(matches) == 1
        assert matches[0].data.model_name == "my_model"


class TestReferenceDeprecatingWarning:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": model__sql,
            "my_dependant_model.sql": dependant_model__sql,
            "my_schema.yml": deprecating_model__yml,
        }

    def test_deprecation_warning(self, project):
        events = []
        dbtRunner(callbacks=[events.append]).invoke(["parse"])
        matches = list([e for e in events if e.info.name == "UpcomingReferenceDeprecation"])
        assert len(matches) == 1
        assert matches[0].data.model_name == "my_dependant_model"
        assert matches[0].data.ref_model_name == "my_model"


class TestReferenceDeprecatedWarning:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": model__sql,
            "my_dependant_model.sql": dependant_model__sql,
            "my_schema.yml": deprecated_model__yml,
        }

    def test_deprecation_warning(self, project):
        events = []
        dbtRunner(callbacks=[events.append]).invoke(["parse"])
        matches = list([e for e in events if e.info.name == "DeprecatedReference"])
        assert len(matches) == 1
        assert matches[0].data.model_name == "my_dependant_model"
        assert matches[0].data.ref_model_name == "my_model"
