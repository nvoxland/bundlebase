import bundlebase.sync as bb

import shutil
from pathlib import Path
import sys

shutil.rmtree("jeoparady")


bundle = bb.create("jeoparady")
bundle.create_source("kaggle", {"dataset": "tunguz/200000-jeopardy-questions"})

bundle.commit("Attached raw data")

bundle.normalize_column_names()

bundle.cast_column("value", "integer")

# bundle.create_index("round", "btree")
# bundle.create_index("category", "btree")
# bundle.create_index("value", "btree")
bundle.create_index(["question", "answer"], "text")

bundle.rename_column("question", "q")

bundle.add_column("big_val", "value > 400")

bundle.commit("Indexed data")
