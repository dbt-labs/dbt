import dataclasses
import os
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from metricflow.converters.osi_to_msi import OSIToMSIConverter

from dbt.constants import SUPPORTED_OSI_VERSIONS
from dbt.contracts.files import OsiSourceFile
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import Metric, ModelNode, SemanticModel
from dbt.events.types import MFConverterIssue
from dbt.exceptions import ParsingError
from dbt.node_types import NodeType
from dbt_common.events.functions import fire_event


@dataclasses.dataclass
class _OsiFileContext:
    path: Path
    rel_path: str
    now: float
    package_name: str

    @property
    def file_id(self) -> str:
        return f"{self.package_name}://{self.rel_path}"


def _scan_osi_directories(project_root: str, osi_paths: List[str]) -> List[Path]:
    files: List[Path] = []
    seen: set = set()
    for osi_path in osi_paths:
        osi_dir = Path(project_root) / osi_path
        if not osi_dir.is_dir():
            continue
        stat = osi_dir.stat()
        dir_id = (stat.st_dev, stat.st_ino)
        if dir_id in seen:
            continue
        seen.add(dir_id)
        files.extend(osi_dir.rglob("*.json"))
    return sorted(files)


def _build_model_lookup(manifest: Manifest) -> Dict[Tuple[str, str, str], ModelNode]:
    return {
        (
            (node.alias or node.name).lower(),
            node.schema.lower(),
            (node.database or "").lower(),
        ): node
        for node in manifest.nodes.values()
        if isinstance(node, ModelNode)
    }


def _match_database_less(
    error_prefix: str,
    key: Tuple[str, str, str],
    model_lookup: Dict[Tuple[str, str, str], ModelNode],
) -> Optional[ModelNode]:
    # Database-less sources (`schema.table`) bind on schema + alias alone, so a
    # single OSI document can be shared across environments whose only
    # difference is the database (e.g. per-environment BigQuery projects).
    candidates = [
        node for (alias, schema, _), node in model_lookup.items() if (alias, schema) == key[:2]
    ]
    if len(candidates) > 1:
        databases = sorted({node.database or "<no database>" for node in candidates})
        raise ParsingError(
            f"{error_prefix} matches models in multiple "
            f"databases: {', '.join(databases)}. Qualify the source with a "
            f"database to disambiguate."
        )
    return candidates[0] if candidates else None


def _match_model_for_dataset(
    ctx: _OsiFileContext,
    dataset_name: str,
    node_relation: Any,
    model_lookup: Dict[Tuple[str, str, str], ModelNode],
) -> ModelNode:
    key = (
        (node_relation.alias or "").lower(),
        (node_relation.schema_name or "").lower(),
        (node_relation.database or "").lower(),
    )
    table_ref = ".".join(
        filter(None, [node_relation.database, node_relation.schema_name, node_relation.alias])
    )
    error_prefix = f"OSI file '{ctx.path}' contains dataset '{dataset_name}' ({table_ref}) that"

    matched = model_lookup.get(key)
    if matched is None and not key[2]:
        matched = _match_database_less(error_prefix, key, model_lookup)
    if matched is None:
        raise ParsingError(
            f"{error_prefix} does not match any dbt model in this project. "
            f"Each OSI dataset must reference a table managed by a dbt model."
        )
    return matched


def _inject_one_semantic_model(
    manifest: Manifest,
    ctx: _OsiFileContext,
    model_lookup: Dict[Tuple[str, str, str], ModelNode],
    pydantic_sm: Any,
) -> str:
    nr = pydantic_sm.node_relation
    matched = _match_model_for_dataset(ctx, pydantic_sm.name, nr, model_lookup)

    unique_id = f"semantic_model.{ctx.package_name}.{pydantic_sm.name}"
    if unique_id in manifest.semantic_models:
        raise ParsingError(
            f"OSI file '{ctx.path}' defines semantic model '{pydantic_sm.name}' "
            f"which conflicts with an existing semantic model in this project."
        )

    d = pydantic_sm.dict()
    d.pop("config", None)
    d.pop("metadata", None)
    d.update(
        {
            "resource_type": NodeType.SemanticModel,
            "package_name": ctx.package_name,
            "path": ctx.rel_path,
            "original_file_path": ctx.rel_path,
            "unique_id": unique_id,
            "fqn": [ctx.package_name, pydantic_sm.name],
            "model": f"ref('{matched.name}')",
            "node_relation": None,
            "refs": [{"name": matched.name, "package": None, "version": None}],
            "created_at": ctx.now,
        }
    )
    manifest.semantic_models[unique_id] = SemanticModel.from_dict(d)
    return unique_id


def _inject_one_metric(
    manifest: Manifest,
    ctx: _OsiFileContext,
    pydantic_metric: Any,
) -> str:
    unique_id = f"metric.{ctx.package_name}.{pydantic_metric.name}"
    if unique_id in manifest.metrics:
        raise ParsingError(
            f"OSI file '{ctx.path}' defines metric '{pydantic_metric.name}' "
            f"which conflicts with an existing metric in this project."
        )

    d = pydantic_metric.dict()
    d.pop("config", None)
    d.pop("metadata", None)
    d.update(
        {
            "resource_type": NodeType.Metric,
            "package_name": ctx.package_name,
            "path": ctx.rel_path,
            "original_file_path": ctx.rel_path,
            "unique_id": unique_id,
            "fqn": [ctx.package_name, pydantic_metric.name],
            "description": pydantic_metric.description or "",
            "label": pydantic_metric.label or pydantic_metric.name,
            "created_at": ctx.now,
        }
    )
    manifest.metrics[unique_id] = Metric.from_dict(d)
    return unique_id


def _process_osi_file(
    ctx: _OsiFileContext,
    manifest: Manifest,
    model_lookup: Dict[Tuple[str, str, str], ModelNode],
) -> None:
    from metricflow.converters.models import OSIDocument

    try:
        doc = OSIDocument.parse_raw(ctx.path.read_text(encoding="utf-8"))
    except Exception as exc:
        raise ParsingError(f"Failed to parse OSI file '{ctx.path}': {exc}") from exc

    if doc.version not in SUPPORTED_OSI_VERSIONS:
        raise ParsingError(
            f"OSI file '{ctx.path}' uses unsupported version '{doc.version}'. "
            f"Supported versions: {sorted(SUPPORTED_OSI_VERSIONS)}"
        )

    result = OSIToMSIConverter().convert(doc)

    for issue in result.issues:
        fire_event(
            MFConverterIssue(
                issue_type=issue.issue_type.value,
                element_name=issue.element_name,
                converter_name="OSIToMSIConverter",
            )
        )

    sm_ids = [
        _inject_one_semantic_model(manifest, ctx, model_lookup, sm)
        for sm in result.output.semantic_models
    ]
    metric_ids = [_inject_one_metric(manifest, ctx, m) for m in result.output.metrics]
    _attribute_to_source_file(manifest, ctx.file_id, sm_ids, metric_ids)


def _attribute_to_source_file(
    manifest: Manifest,
    file_id: str,
    sm_ids: List[str],
    metric_ids: List[str],
) -> None:
    sf = manifest.files.get(file_id)
    if isinstance(sf, OsiSourceFile):
        sf.semantic_models.extend(sm_ids)
        sf.metrics.extend(metric_ids)


def _clear_osi_attributed_nodes(manifest: Manifest, osi_paths: List[str]) -> None:
    osi_prefixes = tuple(p + os.sep for p in osi_paths)
    for uid in [
        u
        for u, n in manifest.semantic_models.items()
        if n.original_file_path.startswith(osi_prefixes)
    ]:
        del manifest.semantic_models[uid]
    for uid in [
        u for u, n in manifest.metrics.items() if n.original_file_path.startswith(osi_prefixes)
    ]:
        del manifest.metrics[uid]
    for sf in manifest.files.values():
        if isinstance(sf, OsiSourceFile):
            sf.semantic_models.clear()
            sf.metrics.clear()


def load_osi_into_manifest(
    project_root: str,
    package_name: str,
    manifest: Manifest,
    osi_paths: List[str],
) -> None:
    # Clear any OSI-attributed nodes from a prior load (e.g., partial parse replay).
    # Must run before the early-return so deleted OSI files don't leave stale nodes.
    _clear_osi_attributed_nodes(manifest, osi_paths)

    files = _scan_osi_directories(project_root, osi_paths)
    if not files:
        return

    project_root_path = Path(project_root)
    model_lookup = _build_model_lookup(manifest)
    now = time.time()

    for path in files:
        ctx = _OsiFileContext(
            path=path,
            rel_path=str(path.relative_to(project_root_path)),
            now=now,
            package_name=package_name,
        )
        _process_osi_file(ctx, manifest, model_lookup)
