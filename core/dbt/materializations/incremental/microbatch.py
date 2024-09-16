from datetime import datetime, timedelta
from typing import List, Optional, Tuple

import pytz

from dbt.artifacts.resources.types import BatchSize
from dbt.contracts.graph.nodes import ModelNode, NodeConfig
from dbt.exceptions import DbtInternalError, DbtRuntimeError


class MicrobatchBuilder:
    def __init__(
        self,
        model: ModelNode,
        is_incremental: bool,
        event_time_start: Optional[datetime],
        event_time_end: Optional[datetime],
    ):
        if model.config.incremental_strategy != "microbatch":
            raise DbtInternalError(
                f"Model '{model.name}' does not use 'microbatch' incremental_strategy."
            )
        self.model = model

        if self.model.config.batch_size is None:
            raise DbtRuntimeError(
                f"Microbatch model '{self.model.name}' does not have a 'batch_size' config (one of {[batch_size.value for batch_size in BatchSize]}) specificed."
            )

        self.is_incremental = is_incremental
        self.event_time_start = (
            event_time_start.replace(tzinfo=pytz.UTC) if event_time_start else None
        )
        self.event_time_end = event_time_end.replace(tzinfo=pytz.UTC) if event_time_end else None

    def build_end_time(self):
        return self.event_time_end or datetime.now(tz=pytz.utc)

    def build_start_time(self, checkpoint: Optional[datetime]):
        if self.event_time_start:
            return MicrobatchBuilder.truncate_timestamp(
                self.event_time_start, self.model.config.batch_size
            )

        if not self.is_incremental or checkpoint is None:
            # TODO: return new model-level configuration or raise error
            return None

        assert isinstance(self.model.config, NodeConfig)
        batch_size = self.model.config.batch_size

        lookback = self.model.config.lookback
        start = MicrobatchBuilder.offset_timestamp(checkpoint, batch_size, -1 * lookback)

        return start

    def build_batches(
        self, start: Optional[datetime], end: datetime
    ) -> List[Tuple[Optional[datetime], datetime]]:
        """
        Given a start and end datetime, builds a list of batches where each batch is
        the size of the model's batch_size.
        """
        if start is None:
            return [(start, end)]

        batch_size = self.model.config.batch_size
        curr_batch_start: datetime = start
        curr_batch_end: datetime = MicrobatchBuilder.offset_timestamp(
            curr_batch_start, batch_size, 1
        )

        batches: List[Tuple[Optional[datetime], datetime]] = [(curr_batch_start, curr_batch_end)]
        while curr_batch_end <= end:
            curr_batch_start = curr_batch_end
            curr_batch_end = MicrobatchBuilder.offset_timestamp(curr_batch_start, batch_size, 1)
            batches.append((curr_batch_start, curr_batch_end))

        # use exact end value as stop
        batches[-1] = (batches[-1][0], end)

        return batches

    @staticmethod
    def offset_timestamp(timestamp: datetime, batch_size: BatchSize, offset: int) -> datetime:
        truncated = MicrobatchBuilder.truncate_timestamp(timestamp, batch_size)
        if batch_size == BatchSize.hour:
            offset_timestamp = truncated + timedelta(hours=offset)
        elif batch_size == BatchSize.day:
            offset_timestamp = truncated + timedelta(days=offset)
        elif batch_size == BatchSize.month:
            for _ in range(offset):
                truncated = timestamp + timedelta(days=1)
                truncated = datetime(truncated.year, truncated.month, 1, 0, 0, 0, 0, pytz.utc)
        elif batch_size == BatchSize.year:
            offset_timestamp = truncated.replace(year=truncated.year + offset)

        return offset_timestamp

    @staticmethod
    def truncate_timestamp(timestamp: datetime, batch_size: BatchSize):
        if batch_size == BatchSize.hour:
            truncated = datetime(
                timestamp.year,
                timestamp.month,
                timestamp.day,
                timestamp.hour,
                0,
                0,
                0,
                pytz.utc,
            )
        elif batch_size == BatchSize.day:
            truncated = datetime(
                timestamp.year, timestamp.month, timestamp.day, 0, 0, 0, 0, pytz.utc
            )
        elif batch_size == BatchSize.month:
            truncated = datetime(timestamp.year, timestamp.month, 1, 0, 0, 0, 0, pytz.utc)
        elif batch_size == BatchSize.year:
            truncated = datetime(timestamp.year, 1, 1, 0, 0, 0, 0, pytz.utc)

        return truncated
