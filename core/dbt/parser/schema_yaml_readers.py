from collections.abc import Sequence
from typing import Any, Dict, List, Optional, Union

from metricflow_semantic_interfaces.type_enums import (
    AggregationType,
    ConversionCalculationType,
    DimensionType,
    EntityType,
    MetricType,
    PeriodAggregation,
    TimeGranularity,
)

from dbt.artifacts.resources import (
    ColumnDimension,
    ColumnEntity,
    ColumnInfo,
    ConversionTypeParams,
    CumulativeTypeParams,
    Defaults,
    Dimension,
    DimensionTypeParams,
    DimensionValidityParams,
    Entity,
    Export,
    ExportConfig,
    ExposureConfig,
    Measure,
    MeasureAggregationParameters,
    MetricAggregationParams,
    MetricConfig,
    MetricInput,
    MetricInputMeasure,
    MetricTimeWindow,
    MetricTypeParams,
    NonAdditiveDimension,
    QueryParams,
    SavedQueryConfig,
    SemanticLayerElementConfig,
    WhereFilter,
    WhereFilterIntersection,
)
from dbt.clients.jinja import get_rendered
from dbt.context.context_config import (
    BaseContextConfigGenerator,
    ContextConfigGenerator,
    UnrenderedConfigGenerator,
)
from dbt.context.providers import (
    generate_parse_exposure,
    generate_parse_semantic_models,
)
from dbt.contracts.files import SchemaSourceFile
from dbt.contracts.graph.nodes import (
    Exposure,
    Group,
    Metric,
    ModelNode,
    ParsedNodePatch,
    SavedQuery,
    SemanticModel,
)
from dbt.contracts.graph.unparsed import (
    PercentileType,
    UnparsedConversionTypeParams,
    UnparsedCumulativeTypeParams,
    UnparsedDerivedDimensionV2,
    UnparsedDerivedSemantics,
    UnparsedDimension,
    UnparsedDimensionTypeParams,
    UnparsedEntity,
    UnparsedExport,
    UnparsedExposure,
    UnparsedGroup,
    UnparsedMeasure,
    UnparsedMetric,
    UnparsedMetricBase,
    UnparsedMetricInput,
    UnparsedMetricInputMeasure,
    UnparsedMetricTypeParams,
    UnparsedMetricV2,
    UnparsedNonAdditiveDimension,
    UnparsedNonAdditiveDimensionV2,
    UnparsedQueryParams,
    UnparsedSavedQuery,
    UnparsedSemanticModel,
    UnparsedSemanticModelConfig,
    UnparsedSemanticResourceConfig,
)
from dbt.events.types import ValidationWarning
from dbt.exceptions import JSONValidationError, YamlParseDictError
from dbt.node_types import NodeType
from dbt.parser.common import YamlBlock, normalize_tags
from dbt.parser.schemas import ParseResult, SchemaParser, YamlReader
from dbt_common.dataclass_schema import ValidationError
from dbt_common.exceptions import DbtInternalError
from dbt_common.events.functions import warn_or_error


def parse_where_filter(
    where: Optional[Union[List[str], str]],
) -> Optional[WhereFilterIntersection]:
    if where is None:
        return None
    elif isinstance(where, str):
        return WhereFilterIntersection([WhereFilter(where)])
    else:
        return WhereFilterIntersection([WhereFilter(where_str) for where_str in where])


class ExposureParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, NodeType.Exposure.pluralize())
        self.schema_parser = schema_parser
        self.yaml = yaml

    def parse_exposure(self, unparsed: UnparsedExposure) -> None:
        package_name = self.project.project_name
        unique_id = f"{NodeType.Exposure}.{package_name}.{unparsed.name}"
        path = self.yaml.path.relative_path

        fqn = self.schema_parser.get_fqn_prefix(path)
        fqn.append(unparsed.name)

        config = self._generate_exposure_config(
            target=unparsed,
            fqn=fqn,
            package_name=package_name,
            rendered=True,
        )

        config = config.finalize_and_validate()

        unrendered_config = self._generate_exposure_config(
            target=unparsed,
            fqn=fqn,
            package_name=package_name,
            rendered=False,
        )

        if not isinstance(config, ExposureConfig):
            raise DbtInternalError(
                f"Calculated a {type(config)} for an exposure, but expected an ExposureConfig"
            )

        tags = normalize_tags(self.project.exposures.get("tags"), unparsed.tags, config.tags)
        meta = {**self.project.exposures.get("meta", {}), **unparsed.meta, **config.meta}

        config.tags = tags
        config.meta = meta

        parsed = Exposure(
            resource_type=NodeType.Exposure,
            package_name=package_name,
            path=path,
            original_file_path=self.yaml.path.original_file_path,
            unique_id=unique_id,
            fqn=fqn,
            name=unparsed.name,
            type=unparsed.type,
            url=unparsed.url,
            meta=meta,
            tags=tags,
            description=unparsed.description,
            label=unparsed.label,
            owner=unparsed.owner,
            maturity=unparsed.maturity,
            config=config,
            unrendered_config=unrendered_config,
        )
        ctx = generate_parse_exposure(
            parsed,
            self.root_project,
            self.schema_parser.manifest,
            package_name,
        )
        depends_on_jinja = "\n".join("{{ " + line + "}}" for line in unparsed.depends_on)
        get_rendered(depends_on_jinja, ctx, parsed, capture_macros=True)
        # parsed now has a populated refs/sources/metrics

        assert isinstance(self.yaml.file, SchemaSourceFile)
        if parsed.config.enabled:
            self.manifest.add_exposure(self.yaml.file, parsed)
        else:
            self.manifest.add_disabled(self.yaml.file, parsed)

    def _generate_exposure_config(
        self, target: UnparsedExposure, fqn: List[str], package_name: str, rendered: bool
    ):
        generator: BaseContextConfigGenerator
        if rendered:
            generator = ContextConfigGenerator(self.root_project)
        else:
            generator = UnrenderedConfigGenerator(self.root_project)

        # configs with precendence set
        precedence_configs = dict()
        # apply exposure configs
        precedence_configs.update(target.config)

        return generator.calculate_node_config(
            config_call_dict={},
            fqn=fqn,
            resource_type=NodeType.Exposure,
            project_name=package_name,
            base=False,
            patch_config_dict=precedence_configs,
        )

    def parse(self) -> None:
        for data in self.get_key_dicts():
            try:
                UnparsedExposure.validate(data)
                unparsed = UnparsedExposure.from_dict(data)
            except (ValidationError, JSONValidationError) as exc:
                raise YamlParseDictError(self.yaml.path, self.key, data, exc)

            self.parse_exposure(unparsed)


class MetricParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, NodeType.Metric.pluralize())
        self.schema_parser = schema_parser
        self.yaml = yaml

    def _get_input_measure(
        self,
        unparsed_input_measure: Union[UnparsedMetricInputMeasure, str],
    ) -> MetricInputMeasure:
        if isinstance(unparsed_input_measure, str):
            return MetricInputMeasure(name=unparsed_input_measure)
        else:
            return MetricInputMeasure(
                name=unparsed_input_measure.name,
                filter=parse_where_filter(unparsed_input_measure.filter),
                alias=unparsed_input_measure.alias,
                join_to_timespine=unparsed_input_measure.join_to_timespine,
                fill_nulls_with=unparsed_input_measure.fill_nulls_with,
            )

    def _get_optional_input_measure(
        self,
        unparsed_input_measure: Optional[Union[UnparsedMetricInputMeasure, str]],
    ) -> Optional[MetricInputMeasure]:
        if unparsed_input_measure is not None:
            return self._get_input_measure(unparsed_input_measure)
        else:
            return None

    def _get_input_measures(
        self,
        unparsed_input_measures: Optional[List[Union[UnparsedMetricInputMeasure, str]]],
    ) -> List[MetricInputMeasure]:
        input_measures: List[MetricInputMeasure] = []
        if unparsed_input_measures is not None:
            for unparsed_input_measure in unparsed_input_measures:
                input_measures.append(self._get_input_measure(unparsed_input_measure))

        return input_measures

    def _get_period_agg(self, unparsed_period_agg: str) -> PeriodAggregation:
        return PeriodAggregation(unparsed_period_agg)

    def _get_optional_time_window(
        self, unparsed_window: Optional[str]
    ) -> Optional[MetricTimeWindow]:
        if unparsed_window is not None:
            parts = unparsed_window.lower().split(" ")
            if len(parts) != 2:
                raise YamlParseDictError(
                    self.yaml.path,
                    "window",
                    {"window": unparsed_window},
                    f"Invalid window ({unparsed_window}) in cumulative/conversion metric. Should be of the form `<count> <granularity>`, "
                    "e.g., `28 days`",
                )

            granularity = parts[1]
            # once we drop python 3.8 this could just be `granularity = parts[0].removesuffix('s')
            if granularity.endswith("s") and granularity[:-1] in [
                item.value for item in TimeGranularity
            ]:
                # Can only remove the `s` if it's a standard grain, months -> month
                granularity = granularity[:-1]

            count = parts[0]
            if not count.isdigit():
                raise YamlParseDictError(
                    self.yaml.path,
                    "window",
                    {"window": unparsed_window},
                    f"Invalid count ({count}) in cumulative/conversion metric window string: ({unparsed_window})",
                )

            return MetricTimeWindow(
                count=int(count),
                granularity=granularity,
            )
        else:
            return None

    def _get_metric_input(self, unparsed: Union[UnparsedMetricInput, str]) -> MetricInput:
        if isinstance(unparsed, str):
            return MetricInput(name=unparsed)
        else:
            return MetricInput(
                name=unparsed.name,
                filter=parse_where_filter(unparsed.filter),
                alias=unparsed.alias,
                offset_window=self._get_optional_time_window(unparsed.offset_window),
                offset_to_grain=unparsed.offset_to_grain,
            )

    def _get_optional_metric_input(
        self,
        unparsed: Optional[Union[UnparsedMetricInput, str]],
    ) -> Optional[MetricInput]:
        if unparsed is not None:
            return self._get_metric_input(unparsed)
        else:
            return None

    def _get_metric_inputs(
        self,
        unparsed_metric_inputs: Optional[List[Union[UnparsedMetricInput, str]]],
    ) -> List[MetricInput]:
        metric_inputs: List[MetricInput] = []
        if unparsed_metric_inputs is not None:
            for unparsed_metric_input in unparsed_metric_inputs:
                metric_inputs.append(self._get_metric_input(unparsed=unparsed_metric_input))

        return metric_inputs

    def _get_optional_v1_conversion_type_params(
        self, unparsed: Optional[UnparsedConversionTypeParams]
    ) -> Optional[ConversionTypeParams]:
        if unparsed is None:
            return None
        if unparsed.base_measure is None:
            raise ValidationError(
                "base_measure is required for conversion metrics that use type_params."
            )
        if unparsed.conversion_measure is None:
            raise ValidationError(
                "conversion_measure is required for conversion metrics that use type_params."
            )
        return ConversionTypeParams(
            base_measure=self._get_input_measure(unparsed.base_measure),
            conversion_measure=self._get_input_measure(unparsed.conversion_measure),
            entity=unparsed.entity,
            calculation=ConversionCalculationType(unparsed.calculation),
            window=self._get_optional_time_window(unparsed.window),
            constant_properties=unparsed.constant_properties,
        )

    def _get_optional_v2_conversion_type_params(
        self,
        unparsed_metric: UnparsedMetricV2,
    ) -> Optional[ConversionTypeParams]:
        if MetricType(unparsed_metric.type) is not MetricType.CONVERSION:
            return None

        if unparsed_metric.base_metric is None:
            raise ValidationError("base_metric is required for conversion metrics.")
        if unparsed_metric.conversion_metric is None:
            raise ValidationError("conversion_metric is required for conversion metrics.")
        if unparsed_metric.entity is None:
            raise ValidationError("entity is required for conversion metrics.")

        return ConversionTypeParams(
            base_metric=self._get_metric_input(unparsed_metric.base_metric),
            conversion_metric=self._get_metric_input(unparsed_metric.conversion_metric),
            entity=unparsed_metric.entity,
            calculation=ConversionCalculationType(unparsed_metric.calculation),
            window=self._get_optional_time_window(unparsed_metric.window),
            constant_properties=unparsed_metric.constant_properties,
        )

    def _get_optional_v1_cumulative_type_params(
        self, unparsed_metric: UnparsedMetric
    ) -> Optional[CumulativeTypeParams]:
        unparsed_type_params = unparsed_metric.type_params
        if unparsed_metric.type.lower() == MetricType.CUMULATIVE.value:
            if not unparsed_type_params.cumulative_type_params:
                unparsed_type_params.cumulative_type_params = UnparsedCumulativeTypeParams()

            if (
                unparsed_type_params.window
                and not unparsed_type_params.cumulative_type_params.window
            ):
                unparsed_type_params.cumulative_type_params.window = unparsed_type_params.window
            if (
                unparsed_type_params.grain_to_date
                and not unparsed_type_params.cumulative_type_params.grain_to_date
            ):
                unparsed_type_params.cumulative_type_params.grain_to_date = (
                    unparsed_type_params.grain_to_date
                )

            return CumulativeTypeParams(
                window=self._get_optional_time_window(
                    unparsed_type_params.cumulative_type_params.window
                ),
                grain_to_date=unparsed_type_params.cumulative_type_params.grain_to_date,
                period_agg=self._get_period_agg(
                    unparsed_type_params.cumulative_type_params.period_agg
                ),
            )

        return None

    def _get_optional_v2_cumulative_type_params(
        self,
        unparsed_metric: UnparsedMetricV2,
    ) -> Optional[CumulativeTypeParams]:
        if MetricType(unparsed_metric.type) is not MetricType.CUMULATIVE:
            return None
        input_metric = unparsed_metric.input_metric
        if input_metric is None:
            raise ValidationError("input_metric is required for cumulative metrics.")
        return CumulativeTypeParams(
            window=self._get_optional_time_window(unparsed_metric.window),
            grain_to_date=unparsed_metric.grain_to_date,
            period_agg=self._get_period_agg(unparsed_metric.period_agg),
            metric=self._get_metric_input(input_metric),
        )

    def _get_v2_non_additive_dimension(
        self,
        unparsed_non_additive_dimension: Optional[UnparsedNonAdditiveDimensionV2],
    ) -> Optional[NonAdditiveDimension]:
        if unparsed_non_additive_dimension is None:
            return None
        return NonAdditiveDimension(
            name=unparsed_non_additive_dimension.name,
            window_choice=AggregationType(unparsed_non_additive_dimension.window_agg),
            window_groupings=unparsed_non_additive_dimension.group_by,
        )

    def _get_metric_type_params(
        self,
        unparsed_metric: UnparsedMetricBase,
        generated_from: Optional[str] = None,
        default_agg_time_dimension: Optional[str] = None,
    ) -> MetricTypeParams:
        if isinstance(unparsed_metric, UnparsedMetric):
            type_params = unparsed_metric.type_params

            grain_to_date: Optional[TimeGranularity] = None
            if type_params.grain_to_date is not None:
                # This should've been changed to a string (to support custom grain), but since this
                # is a legacy field waiting to be deprecated, we will not support custom grain here
                # in order to force customers off of using this field. The field to use should be
                # `cumulative_type_params.grain_to_date`
                grain_to_date = TimeGranularity(type_params.grain_to_date)

            return MetricTypeParams(
                measure=self._get_optional_input_measure(type_params.measure),
                numerator=self._get_optional_metric_input(type_params.numerator),
                denominator=self._get_optional_metric_input(type_params.denominator),
                expr=str(type_params.expr) if type_params.expr is not None else None,
                window=self._get_optional_time_window(type_params.window),
                grain_to_date=grain_to_date,
                metrics=self._get_metric_inputs(type_params.metrics),
                conversion_type_params=self._get_optional_v1_conversion_type_params(
                    type_params.conversion_type_params
                ),
                cumulative_type_params=self._get_optional_v1_cumulative_type_params(
                    unparsed_metric=unparsed_metric,
                ),
                # input measures are calculated via metric processing post parsing
                # input_measures=?,
            )
        elif isinstance(unparsed_metric, UnparsedMetricV2):
            if unparsed_metric.agg is not None:
                if generated_from is None:
                    raise YamlParseDictError(
                        self.yaml.path,
                        self.key,
                        yaml_data=unparsed_metric.to_dict(),
                        cause="simple metrics in v2 YAML must be attached to semantic_model",
                    )
                metric_aggregation_params = MetricAggregationParams(
                    semantic_model=generated_from,
                    agg=AggregationType(unparsed_metric.agg),
                    agg_params=MeasureAggregationParameters(
                        percentile=unparsed_metric.percentile,
                        use_discrete_percentile=(unparsed_metric.percentile_type or "").lower()
                        == PercentileType.DISCRETE,
                        use_approximate_percentile=(unparsed_metric.percentile_type or "").lower()
                        == PercentileType.CONTINUOUS,
                    ),
                    agg_time_dimension=unparsed_metric.agg_time_dimension
                    or default_agg_time_dimension,
                    non_additive_dimension=self._get_v2_non_additive_dimension(
                        unparsed_non_additive_dimension=unparsed_metric.non_additive_dimension,
                    ),
                )
            else:
                metric_aggregation_params = None
            return MetricTypeParams(
                numerator=self._get_optional_metric_input(unparsed_metric.numerator),
                denominator=self._get_optional_metric_input(unparsed_metric.denominator),
                expr=str(unparsed_metric.expr) if unparsed_metric.expr is not None else None,
                window=self._get_optional_time_window(unparsed_metric.window),
                metrics=self._get_metric_inputs(unparsed_metric.input_metrics),
                conversion_type_params=self._get_optional_v2_conversion_type_params(
                    unparsed_metric=unparsed_metric,
                ),
                cumulative_type_params=self._get_optional_v2_cumulative_type_params(
                    unparsed_metric=unparsed_metric,
                ),
                metric_aggregation_params=metric_aggregation_params,
                join_to_timespine=unparsed_metric.join_to_timespine or False,
                is_private=unparsed_metric.hidden,
            )
        else:
            raise DbtInternalError(
                f"Tried to parse type params for a {type(unparsed_metric)}, but expected "
                "an UnparsedMetric or UnparsedMetricV2",
            )

    def parse_metric(
        self,
        unparsed: UnparsedMetricBase,
        generated_from: Optional[str] = None,
        default_agg_time_dimension: Optional[str] = None,
    ) -> None:
        package_name = self.project.project_name
        unique_id = f"{NodeType.Metric}.{package_name}.{unparsed.name}"
        path = self.yaml.path.relative_path

        fqn = self.schema_parser.get_fqn_prefix(path)
        fqn.append(unparsed.name)

        config = self._generate_metric_config(
            target=unparsed,
            fqn=fqn,
            package_name=package_name,
            rendered=True,
        )

        config.tags = normalize_tags(config.tags)

        config = config.finalize_and_validate()

        unrendered_config = self._generate_metric_config(
            target=unparsed,
            fqn=fqn,
            package_name=package_name,
            rendered=False,
        )

        if not isinstance(config, MetricConfig):
            raise DbtInternalError(
                f"Calculated a {type(config)} for a metric, but expected a MetricConfig"
            )

        if isinstance(unparsed, UnparsedMetric):
            # If we have meta in the config, copy to node level, for backwards
            # compatibility with earlier node-only config.
            if "meta" in config and config["meta"]:
                unparsed.meta = config["meta"]
            meta = unparsed.meta
            if unparsed.tags:
                # Top-level tags on v1 metrics are ignored; use config.tags instead.
                warn_or_error(
                    ValidationWarning(
                        field_name=(
                            "top-level `tags:` (ignored; use `config.tags` in YAML "
                            "or `+tags` in dbt_project.yml instead)"
                        ),
                        resource_type="metric (v1)",
                        node_name=unparsed.name,
                    )
                )
        elif isinstance(unparsed, UnparsedMetricV2):
            # V2 Metrics do not have a top-level meta or tags field; use config only.
            meta = {}
        else:
            raise DbtInternalError(
                f"Tried to parse a {type(unparsed)} into a metric, but expected "
                "an UnparsedMetric or UnparsedMetricV2",
            )

        parsed = Metric(
            resource_type=NodeType.Metric,
            package_name=package_name,
            path=path,
            original_file_path=self.yaml.path.original_file_path,
            unique_id=unique_id,
            fqn=fqn,
            name=unparsed.name,
            description=unparsed.description,
            label=unparsed.label or unparsed.name,
            type=MetricType(unparsed.type),
            type_params=self._get_metric_type_params(
                unparsed,
                generated_from=generated_from,
                default_agg_time_dimension=default_agg_time_dimension,
            ),
            time_granularity=unparsed.time_granularity,
            filter=parse_where_filter(unparsed.filter),
            meta=meta,
            tags=config.tags,
            config=config,
            unrendered_config=unrendered_config,
            group=config.group,
        )

        # if the metric is disabled we do not want it included in the manifest, only in the disabled dict
        assert isinstance(self.yaml.file, SchemaSourceFile)
        if parsed.config.enabled:
            self.manifest.add_metric(self.yaml.file, parsed, generated_from)
        else:
            self.manifest.add_disabled(self.yaml.file, parsed)

    def _generate_metric_config(
        self, target: UnparsedMetricBase, fqn: List[str], package_name: str, rendered: bool
    ):
        generator: BaseContextConfigGenerator
        if rendered:
            generator = ContextConfigGenerator(self.root_project)
        else:
            generator = UnrenderedConfigGenerator(self.root_project)

        # configs with precendence set
        precedence_configs = dict()
        # first apply metric configs
        precedence_configs.update(target.config)

        config = generator.calculate_node_config(
            config_call_dict={},
            fqn=fqn,
            resource_type=NodeType.Metric,
            project_name=package_name,
            base=False,
            patch_config_dict=precedence_configs,
        )
        return config

    def _parse_v2_metric(
        self, data: dict[str, Any], semantic_model_name: Optional[str] = None
    ) -> None:
        try:
            UnparsedMetricV2.validate(data)
            unparsed = UnparsedMetricV2.from_dict(data)
        except (ValidationError, JSONValidationError) as exc:
            raise YamlParseDictError(self.yaml.path, self.key, data, exc)
        self.parse_metric(unparsed=unparsed)

    def parse_v2_metrics_from_dbt_model_patch(self, model_patch: ParsedNodePatch) -> None:
        if model_patch.metrics is None:
            return
        # Resolve the semantic model name, respecting custom name overrides
        semantic_model_name = model_patch.name
        if isinstance(model_patch.semantic_model, UnparsedSemanticModelConfig):
            if model_patch.semantic_model.name is not None:
                semantic_model_name = model_patch.semantic_model.name
        for metric in model_patch.metrics:
            is_simple = MetricType(metric.type) == MetricType.SIMPLE
            semantic_model = semantic_model_name if is_simple else None
            self.parse_metric(
                metric,
                generated_from=semantic_model,
                default_agg_time_dimension=model_patch.agg_time_dimension if is_simple else None,
            )

    def parse(self) -> None:
        for data in self.get_key_dicts():
            # The main differentiator of old-style yaml and new-style is "type_params",
            # so if that is missing, we'll assume you're using the newer yaml.
            if "type_params" in data:
                try:
                    UnparsedMetric.validate(data)
                    unparsed = UnparsedMetric.from_dict(data)
                except (ValidationError, JSONValidationError) as exc:
                    raise YamlParseDictError(self.yaml.path, self.key, data, exc)
                self.parse_metric(unparsed)
            else:
                self._parse_v2_metric(data)


class GroupParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, NodeType.Group.pluralize())
        self.schema_parser = schema_parser
        self.yaml = yaml

    def parse_group(self, unparsed: UnparsedGroup) -> None:
        package_name = self.project.project_name
        unique_id = f"{NodeType.Group}.{package_name}.{unparsed.name}"
        path = self.yaml.path.relative_path

        fqn = self.schema_parser.get_fqn_prefix(path)
        fqn.append(unparsed.name)
        config = self._generate_group_config(unparsed, fqn, package_name, True)

        parsed = Group(
            resource_type=NodeType.Group,
            package_name=package_name,
            path=path,
            original_file_path=self.yaml.path.original_file_path,
            unique_id=unique_id,
            name=unparsed.name,
            owner=unparsed.owner,
            description=unparsed.description,
            config=config,
        )

        assert isinstance(self.yaml.file, SchemaSourceFile)
        self.manifest.add_group(self.yaml.file, parsed)

    def parse(self):
        for data in self.get_key_dicts():
            try:
                UnparsedGroup.validate(data)
                unparsed = UnparsedGroup.from_dict(data)
            except (ValidationError, JSONValidationError) as exc:
                raise YamlParseDictError(self.yaml.path, self.key, data, exc)

            self.parse_group(unparsed)

    def _generate_group_config(
        self, target: UnparsedGroup, fqn: List[str], package_name: str, rendered: bool
    ):
        generator: BaseContextConfigGenerator
        if rendered:
            generator = ContextConfigGenerator(self.root_project)
        else:
            generator = UnrenderedConfigGenerator(self.root_project)

        # configs with precendence set
        precedence_configs = dict()
        # first apply metric configs
        precedence_configs.update(target.config)

        config = generator.calculate_node_config(
            config_call_dict={},
            fqn=fqn,
            resource_type=NodeType.Group,
            project_name=package_name,
            base=False,
            patch_config_dict=precedence_configs,
        )
        return config


class SemanticModelParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, "semantic_models")
        self.schema_parser = schema_parser
        self.yaml = yaml

    def _get_dimension_type_params(
        self, unparsed: Optional[UnparsedDimensionTypeParams]
    ) -> Optional[DimensionTypeParams]:
        if unparsed is not None:
            return DimensionTypeParams(
                time_granularity=TimeGranularity(unparsed.time_granularity),
                validity_params=unparsed.validity_params,
            )
        else:
            return None

    def _get_dimensions(self, unparsed_dimensions: List[UnparsedDimension]) -> List[Dimension]:
        dimensions: List[Dimension] = []
        for unparsed in unparsed_dimensions:
            dimensions.append(
                Dimension(
                    name=unparsed.name,
                    type=DimensionType(unparsed.type),
                    description=unparsed.description,
                    label=unparsed.label,
                    is_partition=unparsed.is_partition,
                    type_params=self._get_dimension_type_params(unparsed=unparsed.type_params),
                    expr=unparsed.expr,
                    metadata=None,  # TODO: requires a fair bit of parsing context
                    config=SemanticLayerElementConfig(meta=unparsed.config.get("meta", {})),
                )
            )
        return dimensions

    def _get_entities(self, unparsed_entities: List[UnparsedEntity]) -> List[Entity]:
        entities: List[Entity] = []
        for unparsed in unparsed_entities:
            entities.append(
                Entity(
                    name=unparsed.name,
                    type=EntityType(unparsed.type),
                    description=unparsed.description,
                    label=unparsed.label,
                    role=unparsed.role,
                    expr=unparsed.expr,
                    config=SemanticLayerElementConfig(meta=unparsed.config.get("meta", {})),
                )
            )

        return entities

    def _get_non_additive_dimension(
        self, unparsed: Optional[UnparsedNonAdditiveDimension]
    ) -> Optional[NonAdditiveDimension]:
        if unparsed is not None:
            return NonAdditiveDimension(
                name=unparsed.name,
                window_choice=AggregationType(unparsed.window_choice),
                window_groupings=unparsed.window_groupings,
            )
        else:
            return None

    def _get_measures(self, unparsed_measures: List[UnparsedMeasure]) -> List[Measure]:
        measures: List[Measure] = []
        for unparsed in unparsed_measures:
            measures.append(
                Measure(
                    name=unparsed.name,
                    agg=AggregationType(unparsed.agg),
                    description=unparsed.description,
                    label=unparsed.label,
                    expr=str(unparsed.expr) if unparsed.expr is not None else None,
                    agg_params=unparsed.agg_params,
                    non_additive_dimension=self._get_non_additive_dimension(
                        unparsed.non_additive_dimension
                    ),
                    agg_time_dimension=unparsed.agg_time_dimension,
                    config=SemanticLayerElementConfig(meta=unparsed.config.get("meta", {})),
                )
            )
        return measures

    def _create_metric(
        self,
        measure: UnparsedMeasure,
        enabled: bool,
        semantic_model_name: str,
        meta: Optional[Dict[str, Any]] = None,
    ) -> None:
        config: Dict[str, Any] = {"enabled": enabled}
        if meta is not None:
            # Need to propagate meta to metric from measure during create_metric: True
            config["meta"] = meta
        unparsed_metric = UnparsedMetric(
            name=measure.name,
            label=measure.label or measure.name,
            type="simple",
            type_params=UnparsedMetricTypeParams(
                measure=measure.name, expr=measure.expr or measure.name  # type: ignore
            ),
            description=measure.description or f"Metric created from measure {measure.name}",
            config=config,
        )

        parser = MetricParser(self.schema_parser, yaml=self.yaml)
        parser.parse_metric(unparsed=unparsed_metric, generated_from=semantic_model_name)

    def _generate_semantic_model_config(
        self,
        target_config: Dict[str, Any],
        fqn: List[str],
        package_name: str,
        rendered: bool,
    ):
        generator: BaseContextConfigGenerator
        if rendered:
            generator = ContextConfigGenerator(self.root_project)
        else:
            generator = UnrenderedConfigGenerator(self.root_project)

        # configs with precendence set
        precedence_configs = dict()
        # first apply semantic model configs
        precedence_configs.update(target_config)

        config = generator.calculate_node_config(
            config_call_dict={},
            fqn=fqn,
            resource_type=NodeType.SemanticModel,
            project_name=package_name,
            base=False,
            patch_config_dict=precedence_configs,
        )

        return config

    def _parse_semantic_model_v1(self, unparsed: UnparsedSemanticModel) -> None:
        entities = self._get_entities(unparsed.entities)
        measures = self._get_measures(unparsed.measures)
        dimensions = self._get_dimensions(unparsed.dimensions)

        self._parse_semantic_model_helper(
            semantic_model_name=unparsed.name,
            semantic_model_config=unparsed.config,
            description=unparsed.description,
            label=unparsed.label,
            model=unparsed.model,
            defaults=unparsed.defaults,
            primary_entity=unparsed.primary_entity,
            entities=entities,
            measures=measures,
            unparsed_measures=unparsed.measures,
            dimensions=dimensions,
        )

    def _parse_v2_column_dimensions(self, columns: Dict[str, ColumnInfo]) -> List[Dimension]:
        dimensions: List[Dimension] = []
        for column in columns.values():
            if column.dimension is None:
                continue
            elif isinstance(column.dimension, DimensionType):
                dimensions.append(
                    Dimension(
                        name=column.name,
                        type=column.dimension,
                        description=column.description,
                        metadata=None,  # Not yet supported in v1 or v2 YAML
                        config=SemanticLayerElementConfig(meta=column.config.get("meta", {})),
                    )
                )
            elif isinstance(column.dimension, ColumnDimension):
                type_params = (
                    (
                        DimensionTypeParams(
                            time_granularity=column.granularity,
                            validity_params=(
                                DimensionValidityParams(
                                    is_start=column.dimension.validity_params.is_start,
                                    is_end=column.dimension.validity_params.is_end,
                                )
                                if column.dimension.validity_params is not None
                                else None
                            ),
                        )
                        if column.granularity is not None
                        else None
                    )
                    if column.granularity is not None
                    else None
                )
                meta = dict(column.config.get("meta", {}))
                meta.update((column.dimension.config or {}).get("meta", {}))
                config = SemanticLayerElementConfig(meta=meta)
                dimension_name = column.dimension.name or column.name
                dimensions.append(
                    Dimension(
                        # required
                        type=DimensionType(column.dimension.type),
                        # fields that use column's values as fallback values
                        name=dimension_name,
                        description=column.dimension.description or column.description,
                        config=config,
                        # optional fields
                        label=column.dimension.label,
                        is_partition=column.dimension.is_partition,
                        type_params=type_params,
                        metadata=None,  # Not yet supported in v1 or v2 YAML
                        # When the dimension name differs from the column name, set expr
                        # to the column name so MetricFlow queries the correct warehouse column.
                        expr=column.name if dimension_name != column.name else None,
                    )
                )
        return dimensions

    def _parse_v2_derived_dimensions(
        self,
        derived_dimensions: List[UnparsedDerivedDimensionV2],
    ) -> List[Dimension]:
        dimensions: List[Dimension] = []
        for derived_dimension in derived_dimensions:
            type_params = None
            if derived_dimension.granularity is not None:
                type_params = DimensionTypeParams(
                    time_granularity=TimeGranularity(derived_dimension.granularity),
                    validity_params=(
                        DimensionValidityParams(
                            is_start=derived_dimension.validity_params.is_start,
                            is_end=derived_dimension.validity_params.is_end,
                        )
                        if derived_dimension.validity_params is not None
                        else None
                    ),
                )
            dimensions.append(
                Dimension(
                    type=DimensionType(derived_dimension.type),
                    name=derived_dimension.name,
                    description=derived_dimension.description,
                    label=derived_dimension.label,
                    is_partition=derived_dimension.is_partition,
                    config=SemanticLayerElementConfig(
                        meta=derived_dimension.config.get("meta", {})
                    ),
                    type_params=type_params,
                    # fields unique to derived dimensions
                    expr=derived_dimension.expr,
                )
            )
        return dimensions

    def _parse_v2_column_entities(self, columns: Dict[str, ColumnInfo]) -> List[Entity]:
        entities: List[Entity] = []
        for column in columns.values():
            if column.entity is None:
                continue
            elif isinstance(column.entity, ColumnEntity):
                entities.append(
                    Entity(
                        name=column.entity.name,
                        type=column.entity.type,
                        description=column.entity.description,
                        label=column.entity.label,
                        # When the entity name differs from the column name, set expr
                        # to the column name so MetricFlow queries the correct warehouse column.
                        expr=column.name if column.entity.name != column.name else None,
                        config=SemanticLayerElementConfig(
                            meta=column.entity.config.get("meta", column.config.get("meta", {}))
                        ),
                    )
                )
            elif isinstance(column.entity, EntityType):
                entities.append(
                    Entity(
                        name=column.name,
                        type=column.entity,
                        description=column.description,
                        label=None,  # there's no label to carry through from columns
                        config=SemanticLayerElementConfig(meta=column.config.get("meta", {})),
                    )
                )
        return entities

    def _parse_v2_derived_semantics_entities(
        self, derived_semantics: UnparsedDerivedSemantics
    ) -> List[Entity]:
        entities: List[Entity] = []
        for unparsed_entity in derived_semantics.entities:
            if unparsed_entity.name is None:
                raise ValidationError("Derived entity is missing a required 'name' field.")
            entities.append(
                Entity(
                    name=unparsed_entity.name,
                    type=EntityType(unparsed_entity.type),
                    description=unparsed_entity.description,
                    label=unparsed_entity.label,
                    expr=unparsed_entity.expr,
                    config=SemanticLayerElementConfig(meta=unparsed_entity.config.get("meta", {})),
                )
            )
        return entities

    def parse_v2_semantic_model_from_dbt_model_patch(
        self,
        node: ModelNode,
        patch: ParsedNodePatch,
    ) -> None:
        if patch.semantic_model is None:
            # We shouldn't be calling this method in this case, but for safety
            # and typechecking, we'll return early here.
            return

        dimensions = self._parse_v2_column_dimensions(patch.columns)
        if patch.derived_semantics is not None:
            dimensions.extend(
                self._parse_v2_derived_dimensions(patch.derived_semantics.dimensions)
            )
        entities = self._parse_v2_column_entities(patch.columns)
        if patch.derived_semantics is not None:
            entities.extend(self._parse_v2_derived_semantics_entities(patch.derived_semantics))

        name = node.name
        config: Dict[str, Any] = {}
        if isinstance(patch.semantic_model, UnparsedSemanticModelConfig):
            if patch.semantic_model.name is not None:
                name = patch.semantic_model.name
            if patch.semantic_model.config is not None:
                unparsed_sub_config = patch.semantic_model.config
                if isinstance(unparsed_sub_config, UnparsedSemanticResourceConfig):
                    if unparsed_sub_config.meta is not None:
                        config["meta"] = unparsed_sub_config.meta
                if patch.semantic_model.enabled is not None:
                    config["enabled"] = patch.semantic_model.enabled
                if patch.semantic_model.group is not None:
                    config["group"] = patch.semantic_model.group
        elif isinstance(patch.semantic_model, bool):
            # boolean value just indicates that the model has a semantic model,
            # so nothing to do here.
            pass
        else:
            # this should be unreachable, but just in case
            raise DbtInternalError(f"Invalid semantic model config: {patch.semantic_model}")

        self._parse_semantic_model_helper(
            semantic_model_name=name,
            semantic_model_config=config,
            description=node.description,
            label=None,  # does not seem to be available in v2 YAML, unless it is part of the semantic model config's 'group'?
            model=f"ref('{patch.name}')",
            defaults=Defaults(agg_time_dimension=patch.agg_time_dimension),
            primary_entity=patch.primary_entity,
            entities=entities,
            dimensions=dimensions,
            # Measures are not part of the v2 YAML design.
            measures=[],
            unparsed_measures=[],
        )

    def _parse_semantic_model_helper(
        self,
        semantic_model_name: str,
        semantic_model_config: Dict[str, Any],
        description: Optional[str],
        label: Optional[str],
        model: str,
        defaults,
        primary_entity,
        entities: List[Entity],
        dimensions: List[Dimension],
        measures: List[Measure],  # v1 only
        unparsed_measures: List[UnparsedMeasure] = [],  # v1 only
    ) -> None:
        package_name = self.project.project_name
        unique_id = f"{NodeType.SemanticModel}.{package_name}.{semantic_model_name}"
        path = self.yaml.path.relative_path

        fqn = self.schema_parser.get_fqn_prefix(path)
        fqn.append(semantic_model_name)

        config = self._generate_semantic_model_config(
            target_config=semantic_model_config,
            fqn=fqn,
            package_name=package_name,
            rendered=True,
        )

        # Combine configs according to the behavior documented here https://docs.getdbt.com/reference/configs-and-properties#combining-configs
        elements: Sequence[Union[Dimension, Entity, Measure]] = [
            *dimensions,
            *entities,
            *measures,
        ]
        for element in elements:
            if config is not None:
                if element.config is None:
                    element.config = SemanticLayerElementConfig(meta=config.meta)
                else:
                    element.config.meta = {**config.get("meta", {}), **element.config.meta}

        config = config.finalize_and_validate()

        unrendered_config = self._generate_semantic_model_config(
            target_config=semantic_model_config,
            fqn=fqn,
            package_name=package_name,
            rendered=False,
        )

        parsed = SemanticModel(
            description=description,
            label=label,
            fqn=fqn,
            model=model,
            name=semantic_model_name,
            node_relation=None,  # Resolved from the value of "model" after parsing
            original_file_path=self.yaml.path.original_file_path,
            package_name=package_name,
            path=path,
            resource_type=NodeType.SemanticModel,
            unique_id=unique_id,
            entities=entities,
            measures=measures,
            dimensions=dimensions,
            defaults=defaults,
            primary_entity=primary_entity,
            config=config,
            unrendered_config=unrendered_config,
            group=config.group,
        )

        ctx = generate_parse_semantic_models(
            parsed,
            self.root_project,
            self.schema_parser.manifest,
            package_name,
        )

        if parsed.model is not None:
            model_ref = "{{ " + parsed.model + " }}"
            # This sets the "refs" in the SemanticModel from the SemanticModelRefResolver in context/providers.py
            get_rendered(model_ref, ctx, parsed)

        # if the semantic model is disabled we do not want it included in the manifest,
        # only in the disabled dict
        assert isinstance(self.yaml.file, SchemaSourceFile)
        if parsed.config.enabled:
            self.manifest.add_semantic_model(self.yaml.file, parsed)
        else:
            self.manifest.add_disabled(self.yaml.file, parsed)

        # Create a metric for each measure with `create_metric = True`
        # This is only relevant for v1 SL YAML; v2 does not include measures at all.
        for measure in unparsed_measures:
            if measure.create_metric is True:
                self._create_metric(
                    measure=measure,
                    enabled=parsed.config.enabled,
                    semantic_model_name=parsed.name,
                    meta=config.meta if config is not None else None,
                )

    def parse(self) -> None:
        for data in self.get_key_dicts():
            try:
                UnparsedSemanticModel.validate(data)
                unparsed = UnparsedSemanticModel.from_dict(data)
            except (ValidationError, JSONValidationError) as exc:
                raise YamlParseDictError(self.yaml.path, self.key, data, exc)

            self._parse_semantic_model_v1(unparsed)


class SavedQueryParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, "saved_queries")
        self.schema_parser = schema_parser
        self.yaml = yaml

    def _generate_saved_query_config(
        self, target: UnparsedSavedQuery, fqn: List[str], package_name: str, rendered: bool
    ):
        generator: BaseContextConfigGenerator
        if rendered:
            generator = ContextConfigGenerator(self.root_project)
        else:
            generator = UnrenderedConfigGenerator(self.root_project)

        # configs with precendence set
        precedence_configs = dict()
        # first apply semantic model configs
        precedence_configs.update(target.config)

        config = generator.calculate_node_config(
            config_call_dict={},
            fqn=fqn,
            resource_type=NodeType.SavedQuery,
            project_name=package_name,
            base=False,
            patch_config_dict=precedence_configs,
        )

        return config

    def _get_export_config(
        self, unparsed_export_config: Dict[str, Any], saved_query_config: SavedQueryConfig
    ) -> ExportConfig:
        # Combine the two dictionaries using dictionary unpacking
        # the second dictionary is the one whose keys take priority
        combined = {**saved_query_config.__dict__, **unparsed_export_config}
        # `schema` is the user facing attribute, but for DSI protocol purposes we track it as `schema_name`
        if combined.get("schema") is not None and combined.get("schema_name") is None:
            combined["schema_name"] = combined["schema"]

        return ExportConfig.from_dict(combined)

    def _get_export(
        self, unparsed: UnparsedExport, saved_query_config: SavedQueryConfig
    ) -> Export:
        return Export(
            name=unparsed.name,
            config=self._get_export_config(unparsed.config, saved_query_config),
            unrendered_config=unparsed.config,
        )

    def _get_query_params(self, unparsed: UnparsedQueryParams) -> QueryParams:
        return QueryParams(
            group_by=unparsed.group_by,
            metrics=unparsed.metrics,
            where=parse_where_filter(unparsed.where),
            order_by=unparsed.order_by,
            limit=unparsed.limit,
        )

    def parse_saved_query(self, unparsed: UnparsedSavedQuery) -> None:
        package_name = self.project.project_name
        unique_id = f"{NodeType.SavedQuery}.{package_name}.{unparsed.name}"
        path = self.yaml.path.relative_path

        fqn = self.schema_parser.get_fqn_prefix(path)
        fqn.append(unparsed.name)

        config = self._generate_saved_query_config(
            target=unparsed,
            fqn=fqn,
            package_name=package_name,
            rendered=True,
        )

        config = config.finalize_and_validate()

        unrendered_config = self._generate_saved_query_config(
            target=unparsed,
            fqn=fqn,
            package_name=package_name,
            rendered=False,
        )

        tags = normalize_tags(unparsed.tags, config.get("tags"))

        parsed = SavedQuery(
            description=unparsed.description,
            label=unparsed.label,
            fqn=fqn,
            name=unparsed.name,
            original_file_path=self.yaml.path.original_file_path,
            package_name=package_name,
            path=path,
            resource_type=NodeType.SavedQuery,
            unique_id=unique_id,
            query_params=self._get_query_params(unparsed.query_params),
            exports=[self._get_export(export, config) for export in unparsed.exports],
            config=config,
            unrendered_config=unrendered_config,
            group=config.group,
            tags=tags,
        )

        for export in parsed.exports:
            self.schema_parser.update_parsed_node_relation_names(export, export.config.to_dict())  # type: ignore

            if not export.config.schema_name:
                export.config.schema_name = getattr(export, "schema", None)
            delattr(export, "schema")

            export.config.database = getattr(export, "database", None) or export.config.database
            delattr(export, "database")

            if not export.config.alias:
                export.config.alias = getattr(export, "alias", None)
            delattr(export, "alias")

            delattr(export, "relation_name")

        # Only add thes saved query if it's enabled, otherwise we track it with other diabled nodes
        assert isinstance(self.yaml.file, SchemaSourceFile)
        if parsed.config.enabled:
            self.manifest.add_saved_query(self.yaml.file, parsed)
        else:
            self.manifest.add_disabled(self.yaml.file, parsed)

    def parse(self) -> ParseResult:
        for data in self.get_key_dicts():
            try:
                UnparsedSavedQuery.validate(data)
                unparsed = UnparsedSavedQuery.from_dict(data)
            except (ValidationError, JSONValidationError) as exc:
                raise YamlParseDictError(self.yaml.path, self.key, data, exc)

            self.parse_saved_query(unparsed)

        # The supertype (YamlReader) requires `parse` to return a ParseResult, so
        # we return an empty one because we don't have one to actually return.
        return ParseResult()
