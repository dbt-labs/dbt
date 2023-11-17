from dbt.common.events.base_types import (
    BaseEvent,
    DynamicLevel as CommonDyanicLevel,
    TestLevel as CommonTestLevel,
    DebugLevel as CommonDebugLevel,
    InfoLevel as CommonInfoLevel,
    WarnLevel as CommonWarnLevel,
    ErrorLevel as CommonErrorLevel,
)
from dbt.adapters.events import adapter_types_pb2


class AdapterBaseEvent(BaseEvent):
    PROTO_TYPES_MODULE = adapter_types_pb2


class DynamicLevel(CommonDyanicLevel, AdapterBaseEvent):
    pass


class TestLevel(CommonTestLevel, AdapterBaseEvent):
    pass


class DebugLevel(CommonDebugLevel, AdapterBaseEvent):
    pass


class InfoLevel(CommonInfoLevel, AdapterBaseEvent):
    pass


class WarnLevel(CommonWarnLevel, AdapterBaseEvent):
    pass


class ErrorLevel(CommonErrorLevel, AdapterBaseEvent):
    pass
