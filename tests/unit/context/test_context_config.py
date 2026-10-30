from typing import Any, Dict, List
from unittest import mock

from dbt.context.context_config import BaseContextConfigGenerator
from dbt.node_types import NodeType


def _project_config_levels(model_configs: Dict[str, Any], fqn: List[str]) -> List[Dict[str, Any]]:
    """Run the real `_project_configs` extractor against a fake project tree."""
    src = mock.MagicMock()
    src.get_config_dict.return_value = model_configs
    generator = mock.MagicMock()
    generator.get_config_source.return_value = src
    project = mock.MagicMock()
    return list(
        BaseContextConfigGenerator._project_configs(generator, project, fqn, NodeType.Model)
    )


class TestProjectConfigsNullFolderKeys:
    """Regression for #16290: null dbt_project.yml keys are hierarchy placeholders,
    not legacy bare config values. Treating them as config made every sibling under
    the parent appear state:modified when the null key was populated or deleted.
    """

    def test_null_sibling_key_is_not_copied_into_config(self):
        model_configs = {
            "repro": {
                "parent": {
                    "alpha": {"+tags": ["alpha"]},
                    "beta": None,
                    "gamma": {"+tags": ["gamma"]},
                }
            }
        }
        levels = _project_config_levels(model_configs, ["repro", "parent", "alpha", "a1"])

        # levels: models root, repro, parent, alpha
        parent_level = levels[2]
        assert "beta" not in parent_level
        assert parent_level == {}

        alpha_level = levels[3]
        assert alpha_level == {"tags": ["alpha"]}

    def test_parent_level_unchanged_when_null_key_is_populated(self):
        before = {
            "repro": {
                "parent": {
                    "alpha": {"+tags": ["alpha"]},
                    "beta": None,
                    "gamma": {"+tags": ["gamma"]},
                }
            }
        }
        after = {
            "repro": {
                "parent": {
                    "alpha": {"+tags": ["alpha"]},
                    "beta": {"b1": {"+tags": ["added"]}},
                    "gamma": {"+tags": ["gamma"]},
                }
            }
        }
        fqn = ["repro", "parent", "alpha", "a1"]
        before_parent = _project_config_levels(before, fqn)[2]
        after_parent = _project_config_levels(after, fqn)[2]
        assert before_parent == after_parent == {}

    def test_parent_level_unchanged_when_null_key_is_deleted(self):
        before = {
            "repro": {
                "parent": {
                    "alpha": {"+tags": ["alpha"]},
                    "beta": None,
                    "gamma": {"+tags": ["gamma"]},
                }
            }
        }
        after = {
            "repro": {
                "parent": {
                    "alpha": {"+tags": ["alpha"]},
                    "gamma": {"+tags": ["gamma"]},
                }
            }
        }
        fqn = ["repro", "parent", "gamma", "g1"]
        before_parent = _project_config_levels(before, fqn)[2]
        after_parent = _project_config_levels(after, fqn)[2]
        assert before_parent == after_parent == {}

    def test_legacy_bare_scalar_config_still_extracted(self):
        model_configs = {
            "repro": {
                "parent": {
                    "materialized": "view",
                    "alpha": {"+tags": ["alpha"]},
                }
            }
        }
        levels = _project_config_levels(model_configs, ["repro", "parent", "alpha", "a1"])
        assert levels[2] == {"materialized": "view"}

    def test_plus_prefixed_null_config_still_extracted(self):
        model_configs = {
            "repro": {
                "parent": {
                    "+enabled": None,
                    "alpha": {},
                }
            }
        }
        levels = _project_config_levels(model_configs, ["repro", "parent", "alpha", "a1"])
        assert levels[2] == {"enabled": None}
