import bundlebase.sync as bb

import shutil
from pathlib import Path
import sys

shutil.rmtree("jeoparady")


bundle = bb.create("jeoparady")
bundle.create_source("kaggle", {"dataset": "tunguz/200000-jeopardy-questions"})

bundle.commit("First commit")