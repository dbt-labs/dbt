import multiprocessing
import pickle
from types import SimpleNamespace

import pytest

from dbt.exceptions import (
    AmbiguousAliasError,
    AmbiguousResourceNameRefError,
    DependencyNotFoundError,
    DocTargetNotFoundError,
    DuplicateResourceNameError,
    PatchTargetNotFoundError,
    TargetNotFoundError,
)


def _node(name="m", resource_type="model"):
    return SimpleNamespace(
        name=name,
        unique_id=f"{resource_type}.pkg.{name}",
        original_file_path=f"models/{name}.sql",
        resource_type=resource_type,
        database="db",
        schema="sch",
        alias=name,
    )


EXCEPTIONS = [
    lambda: TargetNotFoundError(node=_node(), target_name="abc", target_kind="model"),
    lambda: TargetNotFoundError(
        node=_node(),
        target_name="abc",
        target_kind="model",
        target_package="other",
        target_version=2,
        disabled=True,
    ),
    lambda: DocTargetNotFoundError(node=_node(), target_doc_name="doc"),
    lambda: PatchTargetNotFoundError(patches={"a": _node("a")}),
    lambda: DependencyNotFoundError(node=_node(), node_description="model", required_pkg="p"),
    lambda: DuplicateResourceNameError(node_1=_node("a"), node_2=_node("a", "seed")),
    lambda: AmbiguousAliasError(node_1=_node("a"), node_2=_node("b")),
    lambda: AmbiguousResourceNameRefError(
        duped_name="a", unique_ids=["model.p1.a", "model.p2.a"], node=_node()
    ),
]


@pytest.mark.parametrize("make", EXCEPTIONS)
def test_keyword_constructed_exceptions_roundtrip_pickle(make):
    original = make()
    restored = pickle.loads(pickle.dumps(original))
    assert type(restored) is type(original)
    assert str(restored) == str(original)


def test_pickle_preserves_instance_state():
    original = TargetNotFoundError(node=_node(), target_name="abc", target_kind="model")
    original.add_node(_node("outer"))
    original.add_node(_node("outermost"))
    restored = pickle.loads(pickle.dumps(original))
    assert restored.node.name == "outermost"
    assert [n.name for n in restored.stack] == ["outer"]
    assert restored.target_name == "abc"


def _raise_target_not_found(_):
    raise TargetNotFoundError(node=_node(), target_name="abc", target_kind="model")


def test_exception_in_multiprocessing_pool_does_not_hang():
    # Before the fix the unpicklable exception killed the pool's result handler thread
    # and pool.apply() hung forever.
    ctx = multiprocessing.get_context("spawn")
    with ctx.Pool(1) as pool:
        result = pool.apply_async(_raise_target_not_found, (None,))
        with pytest.raises(TargetNotFoundError):
            result.get(timeout=60)
