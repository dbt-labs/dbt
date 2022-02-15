import logging
from typing import Union, Optional, List, Any, Dict

class Subject:
    def __init__(self) -> None: ...
    def set_platform(self, value: Any): ...
    def set_user_id(self, user_id: Any): ...
    def set_screen_resolution(self, width: Any, height: Any): ...
    def set_viewport(self, width: Any, height: Any): ...
    def set_color_depth(self, depth: Any): ...
    def set_timezone(self, timezone: Any): ...
    def set_lang(self, lang: Any): ...
    def set_domain_user_id(self, duid: Any): ...
    def set_ip_address(self, ip: Any): ...
    def set_useragent(self, ua: Any): ...
    def set_network_user_id(self, nuid: Any): ...

logger: logging.Logger

class Emitter:
    endpoint: str
    def __init__(
        self,
        endpoint: str,
        protocol: str = ...,
        port: Optional[int] = ...,
        method: str = ...,
        buffer_size: Optional[int] = ...,
        on_success: Optional[Any] = ...,
        on_failure: Optional[Any] = ...,
        byte_limit: Optional[int] = ...,
    ) -> None: ...
    def is_good_status_code(self, status_code: int) -> bool: ...

class Tracker:
    emitters: Union[List[Any], Any] = ...
    subject: Optional[Subject] = ...
    namespace: Optional[str] = ...
    app_id: Optional[str] = ...
    encode_base64: bool = ...
    def __init__(
        self,
        emitters: Union[List[Any], Any],
        subject: Optional[Subject] = ...,
        namespace: Optional[str] = ...,
        app_id: Optional[str] = ...,
        encode_base64: bool = ...,
    ) -> None: ...
    def set_subject(self, subject: Optional[Subject]): ...
    def track_struct_event(
        self,
        category: str,
        action: str,
        label: Optional[str] = None,
        property_: Optional[str] = None,
        value: Optional[float] = None,
        context: Optional[List[Any]] = None,
        tstamp: Optional[Any] = None,
    ): ...
    def flush(self, asynchronous: bool = False): ...

class SelfDescribingJson:
    schema: Any = ...
    data: Any = ...
    def __init__(self, schema: Any, data: Any) -> None: ...
    def to_json(self) -> Dict[str, Any]: ...
    def to_string(self) -> str: ...
