---
seo_title: "Using Bundlebase in Jupyter Notebooks - Bundlebase"
---

# Jupyter Notebooks

When using Bundlebase in a Jupyter notebook, use the sync API with the jupyter extra:

```bash
pip install "bundlebase[jupyter]"
```

Then in your notebook:

```python
import bundlebase.sync as bb

c = (bb.create()
    .attach("data.parquet")
    .filter("SELECT * FROM bundle WHERE active = true"))

# Display results
display(c.to_pandas())
```