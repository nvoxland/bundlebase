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
    """

    location: str
    must_copy: bool = True
    format: str = "parquet"
    version: str = ""

    def to_dict(self) -> dict:
        return {
            "location": self.location,
            "must_copy": self.must_copy,
            "format": self.format,
            "version": self.version,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "Location":
        return cls(
            location=d.get("location", ""),
            must_copy=d.get("must_copy", True),
            format=d.get("format", "parquet"),
            version=d.get("version", ""),
        )


@dataclass
class StableUrl:
    """A stable URL for a data location.

    Attributes:
        url: The stable URL string.
    """

    url: str
