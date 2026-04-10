"""IPC source that generates random data with incrementing IDs."""

import random
import string

import pyarrow as pa

from bundlebase_sdk import Connector, Location, serve


class RandomSource(Connector):
    def __init__(self):
        self._next_id = 1

    def discover(self, attached_locations, **kwargs):
        return [Location(location="random")]

    def data(self, location, **kwargs):
        rows = []
        for _ in range(100):
            rows.append({
                "id": self._next_id,
                "first_num": random.randint(1, 1000),
                "second_num": random.randint(1, 1000),
                "letter": random.choice(string.ascii_uppercase),
            })
            self._next_id += 1
        return pa.Table.from_pylist(rows)


if __name__ == "__main__":
    serve(RandomSource())
