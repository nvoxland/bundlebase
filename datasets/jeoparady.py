import bundlebase.sync as bb

import shutil
from pathlib import Path
import sys

shutil.rmtree("jeoparady")


bundle = bb.create("jeoparady")
bundle.create_source("kaggle", {"dataset": "tunguz/200000-jeopardy-questions"})

bundle.commit("Attached Data")

bundle.create_index("Round", "column")
bundle.create_index("Category", "column")
bundle.create_index("Value", "column")
bundle.create_index("Question", "text")
bundle.create_index("Answer", "text")

bundle.commit("Indexed")
