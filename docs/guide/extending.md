
## From Chain

Containers can extend other containers to build on previous versions:

```python
# Load committed container
base = await Bundlebase.open("/base/container")

# Extend to new directory with new modifications
extended = await base.extend("/extended/container")
await extended.attach("new_data.parquet")
await extended.commit("Added new data")

# Results in manifest:
# {
#   "from": "/base/container",
#   "operations": [...]
# }
```