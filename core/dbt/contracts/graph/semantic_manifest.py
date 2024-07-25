from typing import List, Optional

from dbt.constants import LEGACY_TIME_SPINE_MODEL_NAME
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import ModelNode
from dbt.events.types import SemanticValidationFailure
from dbt.exceptions import ParsingError
from dbt_common.clients.system import write_file
from dbt_common.events.base_types import EventLevel
from dbt_common.events.functions import fire_event
from dbt_semantic_interfaces.implementations.metric import PydanticMetric
from dbt_semantic_interfaces.implementations.node_relation import PydanticNodeRelation
from dbt_semantic_interfaces.implementations.project_configuration import (
    PydanticProjectConfiguration,
)
from dbt_semantic_interfaces.implementations.saved_query import PydanticSavedQuery
from dbt_semantic_interfaces.implementations.semantic_manifest import (
    PydanticSemanticManifest,
)
from dbt_semantic_interfaces.implementations.semantic_model import PydanticSemanticModel
from dbt_semantic_interfaces.implementations.time_spine import (
    PydanticTimeSpine,
    PydanticTimeSpinePrimaryColumn,
)
from dbt_semantic_interfaces.implementations.time_spine_table_configuration import (
    PydanticTimeSpineTableConfiguration as LegacyTimeSpine,
)
from dbt_semantic_interfaces.type_enums import TimeGranularity
from dbt_semantic_interfaces.validations.semantic_manifest_validator import (
    SemanticManifestValidator,
)


class SemanticManifest:
    def __init__(self, manifest: Manifest) -> None:
        self.manifest = manifest

    def validate(self) -> bool:

        # TODO: Enforce this check.
        # if self.manifest.metrics and not self.manifest.semantic_models:
        #    fire_event(
        #        SemanticValidationFailure(
        #            msg="Metrics require semantic models, but none were found."
        #        ),
        #        EventLevel.ERROR,
        #    )
        #    return False

        if not self.manifest.metrics or not self.manifest.semantic_models:
            return True

        semantic_manifest = self._get_pydantic_semantic_manifest()
        validator = SemanticManifestValidator[PydanticSemanticManifest]()
        validation_results = validator.validate_semantic_manifest(semantic_manifest)

        for warning in validation_results.warnings:
            fire_event(SemanticValidationFailure(msg=warning.message))

        for error in validation_results.errors:
            fire_event(SemanticValidationFailure(msg=error.message), EventLevel.ERROR)

        return not validation_results.errors

    def write_json_to_file(self, file_path: str):
        semantic_manifest = self._get_pydantic_semantic_manifest()
        json = semantic_manifest.json()
        write_file(file_path, json)

    def _get_pydantic_semantic_manifest(self) -> PydanticSemanticManifest:
        pydantic_time_spines: List[PydanticTimeSpine] = []
        daily_time_spine: Optional[PydanticTimeSpine] = None
        for node in self.manifest.nodes.values():
            if not (isinstance(node, ModelNode) and node.time_spine):
                continue
            time_spine = node.time_spine
            standard_granularity_column = None
            for column in node.columns.values():
                if column.name == time_spine.standard_granularity_column:
                    standard_granularity_column = column
                    break
            # Assertions needed for type checking
            if not standard_granularity_column:
                raise ParsingError(
                    "Expected to find time spine standard granularity column in model columns, but did not. "
                    "This should have been caught in YAML parsing."
                )
            if not standard_granularity_column.granularity:
                raise ParsingError(
                    "Expected to find granularity set for time spine standard granularity column, but did not. "
                    "This should have been caught in YAML parsing."
                )
            pydantic_time_spine = PydanticTimeSpine(
                node_relation=PydanticNodeRelation(
                    alias=node.alias,
                    schema_name=node.schema,
                    database=node.database,
                    relation_name=node.relation_name,
                ),
                primary_column=PydanticTimeSpinePrimaryColumn(
                    name=time_spine.standard_granularity_column,
                    time_granularity=standard_granularity_column.granularity,
                ),
            )
            pydantic_time_spines.append(pydantic_time_spine)
            if standard_granularity_column.granularity == TimeGranularity.DAY:
                daily_time_spine = pydantic_time_spine

        project_config = PydanticProjectConfiguration(
            time_spine_table_configurations=[], time_spines=pydantic_time_spines
        )
        pydantic_semantic_manifest = PydanticSemanticManifest(
            metrics=[], semantic_models=[], project_configuration=project_config
        )

        for semantic_model in self.manifest.semantic_models.values():
            pydantic_semantic_manifest.semantic_models.append(
                PydanticSemanticModel.parse_obj(semantic_model.to_dict())
            )

        for metric in self.manifest.metrics.values():
            pydantic_semantic_manifest.metrics.append(PydanticMetric.parse_obj(metric.to_dict()))

        for saved_query in self.manifest.saved_queries.values():
            pydantic_semantic_manifest.saved_queries.append(
                PydanticSavedQuery.parse_obj(saved_query.to_dict())
            )

        if self.manifest.semantic_models:
            # If no time spines have been configured AND legacy time spine model does not exist, error.
            legacy_time_spine_model = self.manifest.ref_lookup.find(
                LEGACY_TIME_SPINE_MODEL_NAME, None, None, self.manifest
            )
            if not (daily_time_spine or legacy_time_spine_model):
                raise ParsingError(
                    "The semantic layer requires a time spine model in the project, but none was found. "
                    "Guidance on creating this model can be found on our docs site ("
                    "https://docs.getdbt.com/docs/build/metricflow-time-spine) "  # TODO: update docs link!
                )

            # For backward compatibility: if legacy time spine exists, include it in the manifest.
            if legacy_time_spine_model:
                legacy_time_spine = LegacyTimeSpine(
                    location=legacy_time_spine_model.relation_name,
                    column_name="date_day",
                    grain=TimeGranularity.DAY,
                )
                pydantic_semantic_manifest.project_configuration.time_spine_table_configurations = [
                    legacy_time_spine
                ]

        return pydantic_semantic_manifest
