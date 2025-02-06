import pytest

from dbt.tests.util import run_dbt

tests__from_yaml_sql = """
{% set simplest = (fromyaml('a: 1') == {'a': 1}) %}
{% set nested_data %}
a:
  b:
   - c: 1
     d: 2
   - c: 3
     d: 4
{% endset %}
{% set nested = (fromyaml(nested_data) == {'a': {'b': [{'c': 1, 'd': 2}, {'c': 3, 'd': 4}]}}) %}

(select 'simplest' as name {% if simplest %}limit 0{% endif %})
union all
(select 'nested' as name {% if nested %}limit 0{% endif %})
"""

tests__to_yaml_sql = """
{% set simplest = (toyaml({'a': 1}) == 'a: 1\\n') %}
{% set default_sort = (toyaml({'b': 2, 'a': 1}) == 'b: 2\\na: 1\\n') %}
{% set unsorted = (toyaml({'b': 2, 'a': 1}, sort_keys=False) == 'b: 2\\na: 1\\n') %}
{% set sorted = (toyaml({'b': 2, 'a': 1}, sort_keys=True) == 'a: 1\\nb: 2\\n') %}
{% set yaml_indented = (toyaml({'a': {'ab': 1}, 'b': {'ba':2}}, indent=8) == 'a:\\n        ab: 1\\nb:\\n        ba: 2\\n') %}
{% set json_indented = (tojson({'a': {'ab': 1}, 'b': {'ba':2}}, indent=4) == '{\\n    "a": {\\n        "ab": 1\\n    },\\n    "b": {\\n        "ba": 2\\n    }\\n}') %}
{% set default_results = (toyaml({'a': adapter}, 'failed') == 'failed') %}

(select 'simplest' as name {% if simplest %}limit 0{% endif %})
union all
(select 'default_sort' as name {% if default_sort %}limit 0{% endif %})
union all
(select 'unsorted' as name {% if unsorted %}limit 0{% endif %})
union all
(select 'sorted' as name {% if sorted %}limit 0{% endif %})
union all
(select 'default_results' as name {% if default_results %}limit 0{% endif %})
"""


class TestContextVars:
    # This test has no actual models

    @pytest.fixture(scope="class")
    def tests(self):
        return {"from_yaml.sql": tests__from_yaml_sql, "to_yaml.sql": tests__to_yaml_sql}

    def test_json_data_tests(self, project):
        assert len(run_dbt(["test"])) == 2
