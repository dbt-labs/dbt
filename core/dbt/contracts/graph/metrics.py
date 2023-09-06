from dbt.node_types import NodeType
from dbt.contracts.graph.manifest import Manifest, Metric
from dbt_semantic_interfaces.type_enums import MetricType

from typing import Dict, Iterator, List


DERIVED_METRICS = [MetricType.DERIVED, MetricType.RATIO]
BASE_METRICS = [MetricType.SIMPLE, MetricType.CUMULATIVE]


class MetricReference(object):
    def __init__(self, metric_name, package_name=None):
        self.metric_name = metric_name
        self.package_name = package_name

    def __str__(self):
        return f"{self.metric_name}"


class ResolvedMetricReference(MetricReference):
    """
    Simple proxy over a Metric which delegates property
    lookups to the underlying node. Also adds helper functions
    for working with metrics (ie. __str__ and templating functions)
    """

    def __init__(self, node, manifest, Relation):
        super().__init__(node.name, node.package_name)
        self.node = node
        self.manifest = manifest
        self.Relation = Relation

    def __getattr__(self, key):
        return getattr(self.node, key)

    def __str__(self):
        return f"{self.node.name}"

    @classmethod
    def parent_metrics(cls, metric_node: Metric, manifest: Manifest) -> Iterator[Metric]:
        """For a given metric, yeilds all upstream metrics."""
        yield metric_node

        for parent_unique_id in metric_node.depends_on.nodes:
            node = manifest.metrics.get(parent_unique_id)
            if node and node.resource_type == NodeType.Metric:
                yield from cls.parent_metrics(node, manifest)

    @classmethod
    def parent_metrics_names(cls, metric_node: Metric, manifest: Manifest) -> Iterator[str]:
        """For a given metric, yeilds all upstream metric names"""
        yield metric_node.name

        for parent_unique_id in metric_node.depends_on.nodes:
            node = manifest.metrics.get(parent_unique_id)
            if node and node.resource_type == NodeType.Metric:
                yield from cls.parent_metrics_names(node, manifest)

    @classmethod
    def reverse_dag_parsing(
        cls, metric_node: Metric, manifest: Manifest, metric_depth_count: int
    ) -> Iterator[Dict[str, int]]:
        """For the given metric, yeilds dictionaries having {<metric_name>: <depth_from_initial_metric} of upstream derived metrics.

        This function is intended as a helper function for other metric helper functions.
        """
        if metric_node.type in DERIVED_METRICS:
            yield {metric_node.name: metric_depth_count}
            metric_depth_count = metric_depth_count + 1

        for parent_unique_id in metric_node.depends_on.nodes:
            node = manifest.metrics.get(parent_unique_id)
            if node and node.resource_type == NodeType.Metric and node.type in DERIVED_METRICS:
                yield from cls.reverse_dag_parsing(node, manifest, metric_depth_count)

    def full_metric_dependency(self):
        """Returns a unique list of all upstream metric names."""
        to_return = list(set(self.parent_metrics_names(self.node, self.manifest)))
        return to_return

    def base_metric_dependency(self) -> List[str]:
        """Returns a list of names for all upstream non-derived metrics."""
        in_scope_metrics = list(self.parent_metrics(self.node, self.manifest))

        to_return = []
        for metric in in_scope_metrics:
            if metric.type not in DERIVED_METRICS and metric.name not in to_return:
                to_return.append(metric.name)

        return to_return

    def derived_metric_dependency(self) -> List[str]:
        """Returns a list of names for all upstream derived metrics."""
        in_scope_metrics = list(self.parent_metrics(self.node, self.manifest))

        to_return = []
        for metric in in_scope_metrics:
            if metric.type in DERIVED_METRICS and metric.name not in to_return:
                to_return.append(metric.name)

        return to_return

    def derived_metric_dependency_depth(self):
        """Returns a list of {<metric_name>: <depth_from_initial_metric>} for all upstream metrics."""
        metric_depth_count = 1
        to_return = list(self.reverse_dag_parsing(self.node, self.manifest, metric_depth_count))

        return to_return
