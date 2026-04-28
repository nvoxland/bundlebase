"""Data types for the Bundlebase connector protocol."""

from dataclasses import dataclass, field


@dataclass
class Location:
    """A discovered data location returned from discover().

    Attributes:
        location: Identifier for this data file (e.g., "data/file1.parquet").
        must_copy: Whether the data must be copied into the bundle (default True).
        format: File format (default "parquet").
        version: Version string for change detection (default "").
        num_rows: Row count for this location, or None if unknown. The Rust
            connector parser requires this field to be present (use ``None``
            for "unknown"); the wire form serializes ``None`` as JSON null.
    """

    location: str
    must_copy: bool = True
    format: str = "parquet"
    version: str = ""
    num_rows: int | None = None

    def to_dict(self) -> dict:
        return {
            "location": self.location,
            "must_copy": self.must_copy,
            "format": self.format,
            "version": self.version,
            "num_rows": self.num_rows,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "Location":
        return cls(
            location=d.get("location", ""),
            must_copy=d.get("must_copy", True),
            format=d.get("format", "parquet"),
            version=d.get("version", ""),
            num_rows=d.get("num_rows"),
        )


@dataclass
class StableUrl:
    """A stable URL for a data location.

    Attributes:
        url: The stable URL string.
    """

    url: str
