"""Entry point for running a source function as a subprocess."""

import sys
from typing import IO

from bundlebase_sdk.source import SourceFunction
from bundlebase_sdk.types import Location, StableUrl
from bundlebase_sdk._protocol import (
    read_request,
    write_response,
    write_error,
    write_arrow_ipc,
    normalize_to_batches,
)


def _serve(source: SourceFunction, stdin: IO[bytes], stdout: IO[bytes]) -> None:
    """Run the JSON-RPC serve loop with explicit IO streams (for testing)."""
    while True:
        req = read_request(stdin)
        if req is None:
            break

        method = req.get("method", "")
        req_id = req.get("id")
        params = req.get("params", {})

        try:
            if method == "discover":
                _handle_discover(source, req_id, params, stdout)
            elif method == "data":
                _handle_data(source, req_id, params, stdout)
            elif method == "stable_url":
                _handle_stable_url(source, req_id, params, stdout)
            elif method == "shutdown":
                write_response(stdout, req_id, {"ok": True})
                break
            else:
                write_error(stdout, req_id, -32601, f"Method not found: {method}")
        except Exception as e:
            write_error(stdout, req_id, -32000, str(e))


def _handle_discover(
    source: SourceFunction,
    req_id: int,
    params: dict,
    stdout: IO[bytes],
) -> None:
    attached = params.pop("attached_locations", [])
    locations = source.discover(attached, **params)
    write_response(
        stdout,
        req_id,
        {"locations": [loc.to_dict() for loc in locations]},
    )


def _handle_data(
    source: SourceFunction,
    req_id: int,
    params: dict,
    stdout: IO[bytes],
) -> None:
    loc_dict = params.pop("location", {})
    location = Location.from_dict(loc_dict)

    kwargs = {k: v for k, v in params.items()}
    data = source.data(location, **kwargs)

    write_response(stdout, req_id, {"ok": True})
    batches = normalize_to_batches(data)
    write_arrow_ipc(stdout, batches)


def _handle_stable_url(
    source: SourceFunction,
    req_id: int,
    params: dict,
    stdout: IO[bytes],
) -> None:
    loc_dict = params.pop("location", {})
    location = Location.from_dict(loc_dict)

    kwargs = {k: v for k, v in params.items()}
    result = source.stable_url(location, **kwargs)

    if isinstance(result, StableUrl):
        write_response(stdout, req_id, {"url": result.url})
    else:
        write_response(stdout, req_id, None)


def serve(source: SourceFunction) -> None:
    """Run the source function as a JSON-RPC subprocess.

    Reads requests from stdin and writes responses to stdout.
    This is the main entry point for source function scripts.
    """
    _serve(source, sys.stdin.buffer, sys.stdout.buffer)
