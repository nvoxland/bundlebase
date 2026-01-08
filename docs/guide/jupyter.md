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
    .filter("active = true"))

# Display results
display(c.to_pandas())
```