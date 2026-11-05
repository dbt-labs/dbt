from dbt.artifacts.resources import Exposure
from tests.unit.utils.manifest import make_exposure


def test_exposure_dimensions_round_trip():
    exposure = make_exposure("test", "dash")
    exposure.dimensions = ["customer__country"]
    assert Exposure.from_dict(exposure.to_dict()).dimensions == ["customer__country"]


def test_exposure_without_dimensions_key_defaults_to_empty():
    data = make_exposure("test", "dash").to_dict()
    del data["dimensions"]
    assert Exposure.from_dict(data).dimensions == []
