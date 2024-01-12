# Stubs for logbook._fallback (Python 3)
#
# NOTE: This dynamically typed stub was automatically generated by stubgen.

from typing import Any

def group_reflected_property(name: Any, default: Any, fallback: Any = ...): ...

class _StackBound:
    def __init__(self, obj: Any, push: Any, pop: Any) -> None: ...
    def __enter__(self): ...
    def __exit__(self, exc_type: Any, exc_value: Any, tb: Any) -> None: ...

class StackedObject:
    def push_greenlet(self) -> None: ...
    def pop_greenlet(self) -> None: ...
    def push_context(self) -> None: ...
    def pop_context(self) -> None: ...
    def push_thread(self) -> None: ...
    def pop_thread(self) -> None: ...
    def push_application(self) -> None: ...
    def pop_application(self) -> None: ...
    def __enter__(self): ...
    def __exit__(self, exc_type: Any, exc_value: Any, tb: Any) -> None: ...
    def greenletbound(self, _cls: Any = ...): ...
    def contextbound(self, _cls: Any = ...): ...
    def threadbound(self, _cls: Any = ...): ...
    def applicationbound(self, _cls: Any = ...): ...

class ContextStackManager:
    def __init__(self) -> None: ...
    def iter_context_objects(self): ...
    def push_greenlet(self, obj: Any) -> None: ...
    def pop_greenlet(self): ...
    def push_context(self, obj: Any) -> None: ...
    def pop_context(self): ...
    def push_thread(self, obj: Any) -> None: ...
    def pop_thread(self): ...
    def push_application(self, obj: Any) -> None: ...
    def pop_application(self): ...
