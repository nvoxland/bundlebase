"""Abstract base class for custom source functions."""

from abc import ABC, abstractmethod
from typing import Iterator, Union

import pyarrow as pa

from bundlebase_sdk.types import Location, StableUrl


class SourceFunction(ABC):
    """Base class for implementing a custom Bundlebase source function.

    Subclass this and implement discover() and data(). Optionally override
    stable_url() if your source has stable URLs for data locations.
    """

    @abstractmethod
    def discover(
        self, attached_locations: list[str], **kwargs: str
    ) -> list[Location]:
        """Discover available data locations.

        Args:
            attached_locations: Locations already attached to the bundle.
            **kwargs: Extra arguments passed from the source configuration.

        Returns:
            List of discovered locations.
        """
        ...

    @abstractmethod
    def data(
        self, location: Location, **kwargs: str
    ) -> Union[
        pa.Table,
        pa.RecordBatch,
        list[pa.RecordBatch],
        list[dict],
        Iterator[dict],
        None,
    ]:
        """Return data for the given location.

        Args:
            location: The location to fetch data for.
            **kwargs: Extra arguments passed from the source configuration.

        Returns:
            Data as a PyArrow Table, RecordBatch, list of RecordBatches,
            list of dicts, iterator of dicts, or None for no data.
        """
        ...

    def stable_url(self, location: Location, **kwargs: str) -> Union[StableUrl, None]:
        """Return a stable URL for the given location, if available.

        Override this method if your source provides stable URLs.
        Default returns None.
        """
        return None
