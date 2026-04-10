import bundlebase.sync as bb

import shutil
from pathlib import Path
import sys

bundle = bb.open("jeoparady")

# print(bundle.query('select * from bundle where "Show Number"=4830').to_pandas())

# print(bundle.query('select big_val from bundle where show_number=4830 and round=\'Jeopardy!\''))

# print(bundle.query(sql='select Round, Value from bundle where Value=\'$200\' and Round=\'Jeopardy!\''))

print(bundle.query(sql="select q, big_val from search('question:moorhead') where _score > 8"))