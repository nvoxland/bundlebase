import bundlebase.sync as bb

import shutil
from pathlib import Path

shutil.rmtree("random-data")

bundle = bb.create("random-data")

# Define a connector and bind it to a Python source class
bundle.create_connector("example.random_data")
bundle.set_temporary_connector_logic("example.random_data", "python", "random_source:RandomSource")
bundle.create_source("example.random_data", {})
bundle.commit("First commit")

bundle.fetch(mode="add")
bundle.commit("Second commit")
