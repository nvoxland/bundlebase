import bundlebase.sync as bb

import shutil
from pathlib import Path

shutil.rmtree("random-data")

bundle = bb.create("random-data")

# Use native (in-process) source - zero-copy Arrow transfer, no subprocess
from random_source import RandomSource
bundle.create_source_native(RandomSource())
bundle.commit("First commit")

bundle.fetch(mode="add")
bundle.commit("Second commit")
