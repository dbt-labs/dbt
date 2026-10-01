from typing import Any
from . import Environment
from typing_extensions import final

@final
class State:
    name: str
    env: Environment
    current_block: str | None
    auto_escape: bool

    def lookup(self, name: str) -> Any: ...
