import bundlebase.sync as bb

import shutil
from pathlib import Path
import sys

shutil.rmtree("random-data")

bundle = bb.create("random-data")
bundle.create_source("ipc", {"call": "python:random_source.py"})
bundle.commit("First commit")

bundle.fetch(mode="add")
bundle.commit("Second commit")
