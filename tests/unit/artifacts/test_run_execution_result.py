from dateutil.tz import tzutc
from hypothesis import given
from hypothesis.strategies import (
    builds,
    composite,
    dictionaries,
    floats,
    lists,
    sampled_from,
    text,
)

from dbt.contracts.results import RunExecutionResult, RunResult, RunStatus, TimingInfo
from tests.unit.fixtures import model_node


@composite
def run_result_strategy(draw):
    node = model_node()
    status = draw(sampled_from(list(RunStatus)))
    message = draw(text(min_size=0, max_size=30) | sampled_from([None]))
    result = RunResult.from_node(node=node, status=status, message=message)
    result.execution_time = draw(floats(min_value=0.0, max_value=30.0))
    result.timing = draw(lists(builds(TimingInfo), max_size=2))

    return result


@given(
    args=dictionaries(text(min_size=1, max_size=10), text(min_size=1, max_size=10)),
    elapsed_time=floats(min_value=0.0, max_value=30.0),
    results=lists(run_result_strategy(), min_size=1, max_size=3),
)
def test_run_execution_result_serialization(args, elapsed_time, results):

    obj = RunExecutionResult(results=results, elapsed_time=elapsed_time, args=args)
    obj_from_dict = RunExecutionResult.from_dict(obj.to_dict())

    assert obj_from_dict.args == obj.args
    assert len(obj_from_dict.results) == len(obj.results)

    assert obj.generated_at.tzinfo is None
    assert obj_from_dict.generated_at.tzinfo == tzutc()
    assert obj_from_dict.generated_at.replace(tzinfo=None) == obj.generated_at

    for original, deserialized in zip(obj.results, obj_from_dict.results):
        assert original.node.created_at == deserialized.node.created_at


def _run_result(state_decision_id=None):
    result = RunResult.from_node(node=model_node(), status=RunStatus.Success, message=None)
    result.state_decision_id = state_decision_id
    return result


def test_to_msg_dict_includes_state_decision_id_when_set():
    msg = _run_result(state_decision_id="abc-123").to_msg_dict()
    assert msg["state_decision_id"] == "abc-123"


def test_to_msg_dict_omits_state_decision_id_when_none():
    msg = _run_result(state_decision_id=None).to_msg_dict()
    assert "state_decision_id" not in msg


def test_state_decision_id_serialization_round_trip():
    result = _run_result(state_decision_id="abc-123")
    obj = RunExecutionResult(results=[result], elapsed_time=1.0, args={})
    obj_from_dict = RunExecutionResult.from_dict(obj.to_dict())
    assert obj_from_dict.results[0].state_decision_id == "abc-123"
