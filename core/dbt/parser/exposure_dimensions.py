from dataclasses import dataclass
from typing import Iterable, List, Optional, Set

from metricflow_semantic_interfaces.type_enums import (
    DimensionType,
    EntityType,
    TimeGranularity,
)

from dbt.contracts.graph.nodes import ModelNode, SemanticModel
from dbt.exceptions import ParsingError

METRIC_TIME = "metric_time"
LINKABLE_ENTITY_TYPES = (EntityType.PRIMARY, EntityType.UNIQUE, EntityType.NATURAL)


@dataclass(frozen=True)
class ExposureDimensionSpec:
    entity_path: List[str]
    dimension: str
    grain: Optional[str] = None


def split_exposure_dimension(specifier: str) -> List[str]:
    tokens = specifier.split("__")
    if any(token == "" for token in tokens):
        raise ParsingError(
            f"dimension('{specifier}') is invalid: empty name between or around '__' separators"
        )
    if METRIC_TIME in tokens:
        raise ParsingError(
            f"dimension('{specifier}') is invalid: '{METRIC_TIME}' is not defined by any "
            "semantic model, so an exposure cannot depend on it"
        )
    if len(tokens) < 2:
        raise ParsingError(
            f"dimension('{specifier}') is invalid: expected "
            "'<entity>__[<entity>__...]<dimension>[__<granularity>]'"
        )
    return tokens


def granularity_names(nodes: Iterable[object]) -> Set[str]:
    names = {granularity.value for granularity in TimeGranularity}
    for node in nodes:
        if isinstance(node, ModelNode) and node.time_spine:
            names.update(
                custom_granularity.name.lower()
                for custom_granularity in node.time_spine.custom_granularities
            )
    return names


def exposure_dimension_interpretations(
    specifier: str, granularities: Set[str]
) -> List[ExposureDimensionSpec]:
    tokens = split_exposure_dimension(specifier)
    interpretations = [ExposureDimensionSpec(entity_path=tokens[:-1], dimension=tokens[-1])]
    if len(tokens) >= 3 and tokens[-1].lower() in granularities:
        interpretations.append(
            ExposureDimensionSpec(
                entity_path=tokens[:-2], dimension=tokens[-2], grain=tokens[-1].lower()
            )
        )
    return interpretations


def find_semantic_models_for_dimension(
    spec: ExposureDimensionSpec, semantic_models: Iterable[SemanticModel]
) -> List[SemanticModel]:
    owner_entity = spec.entity_path[-1]
    linkable: List[SemanticModel] = []
    foreign: List[SemanticModel] = []
    for semantic_model in semantic_models:
        entity_types = [e.type for e in semantic_model.entities if e.name == owner_entity]
        if not entity_types:
            continue
        if not any(d.name == spec.dimension for d in semantic_model.dimensions):
            continue
        if any(t in LINKABLE_ENTITY_TYPES for t in entity_types):
            linkable.append(semantic_model)
        else:
            foreign.append(semantic_model)
    return sorted(linkable or foreign, key=lambda sm: sm.unique_id)


def resolve_exposure_dimension(
    specifier: str,
    semantic_models: Iterable[SemanticModel],
    granularities: Set[str],
    known_entities: Set[str],
) -> List[SemanticModel]:
    candidates = list(semantic_models)
    for spec in exposure_dimension_interpretations(specifier, granularities):
        if not all(entity in known_entities for entity in spec.entity_path):
            continue
        matches = find_semantic_models_for_dimension(spec, candidates)
        if not matches:
            continue
        if spec.grain is not None and any(
            d.type != DimensionType.TIME
            for sm in matches
            for d in sm.dimensions
            if d.name == spec.dimension
        ):
            raise ParsingError(
                f"dimension('{specifier}') is invalid: a granularity can only be applied to a "
                f"time dimension, but '{spec.dimension}' is not one"
            )
        return matches
    return []
