from contextlib import contextmanager
from cProfile import Profile
from pstats import Stats
from typing import Any, Generator


@contextmanager
def profiler(outfile: str) -> Generator[Any, None, None]:
    try:
        profile = Profile()
        profile.enable()

        yield
    finally:
        profile.disable()
        stats = Stats(profile)
        stats.sort_stats("tottime")
        stats.dump_stats(str(outfile))
