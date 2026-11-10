"""Compat shim: legacy import paths for the artifact schemas."""

from dbt.artifacts.schemas.catalog import CatalogArtifact
from dbt.artifacts.schemas.run import RunResultsArtifact
from dbt.artifacts.schemas.sources import FreshnessResultsArtifact

__all__ = [
    "RunResultsArtifact",
    "CatalogArtifact",
    "FreshnessResultsArtifact",
]
