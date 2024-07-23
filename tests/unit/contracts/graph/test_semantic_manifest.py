import pytest

from dbt.contracts.graph.semantic_manifest import SemanticManifest


# Overwrite the default nodes to construct the manifest
@pytest.fixture
def nodes(metricflow_time_spine_model, time_spines):
    print([metricflow_time_spine_model] + [time_spine.model for time_spine in time_spines])
    return [metricflow_time_spine_model] + [time_spine.model for time_spine in time_spines]


@pytest.fixture
def semantic_models(
    semantic_model,
) -> list:
    return [semantic_model]


@pytest.fixture
def metrics(
    metric,
) -> list:
    return [metric]


class TestSemanticManifest:
    def test_validate(self, manifest):
        sm_manifest = SemanticManifest(manifest)
        assert sm_manifest.validate()
