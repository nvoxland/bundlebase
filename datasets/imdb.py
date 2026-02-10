import bundlebase.sync as bb

import sys

def create():
    bundle = bb.create("imdb")
    bundle.create_source("kaggle", {"dataset": "emanafi/clean-imdb-dataset"})
    bundle.commit("First commit")

def read():
    bundle = bb.open("imdb")
    print(bundle.history())

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: imdb.py <create|read>")
        sys.exit(1)

    command = sys.argv[1]
    if command == "create":
        create()
    elif command == "read":
        read()
    else:
        print(f"Unknown command: {command}")
        print("Usage: imdb.py <create|read>")
        sys.exit(1)