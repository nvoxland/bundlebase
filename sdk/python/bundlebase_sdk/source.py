"""Abstract base class for connectors."""

from abc import ABC, abstractmethod
from typing import Iterator, Union

import pyarrow as pa

from bundlebase_sdk.types import Location, StableUrl


class Connector(ABC):
    """Base class for implementing a custom Bundlebase connector.

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

    def schema(self) -> dict[str, str] | None:
        """Optional schema for automatic dict-to-Arrow conversion.

        Return a dict mapping column names to type strings.
        Supported types: Boolean, Int8-64, UInt8-64, Float16/32/64, Utf8, LargeUtf8, Binary, LargeBinary, Date32, Date64, Timestamp.
        Example: {"name": "Utf8", "age": "Int32", "score": "Float64"}
        """
        return None

    @abstractmethod
    def data(
        self, location: Location, **kwargs: str
    ) -> Union[
        pa.Table,
        pa.RecordBatch,
        list[pa.RecordBatch],
        list[dict],
        Iterator[dict],
        dict[str, list],
        None,
    ]:
        """Return data for the given location.

        Args:
            location: The location to fetch data for.
            **kwargs: Extra arguments passed from the source configuration.
                Reserved keys (prefixed with ``_``) may be present:

                - ``_columns``: Comma-separated column names the caller wants.
                  Connectors that support column pushdown can parse this to
                  return only the requested columns. It is safe to ignore.

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
    