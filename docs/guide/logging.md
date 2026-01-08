# Logging and Metrics

## Logging

Bundlebase logs through the standard python logging system under the `bundlebase` logger.

```python
logging.basicConfig(
  stream=sys.stdout,
  level=logging.INFO,
  format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logging.getLogger("bundlebase").setLevel(logging.DEBUG)
```

## Progress Monitoring

Bundlebase provides a pluggable progress tracking system for long-running operations. 

If [tqdm](https://github.com/tqdm/tqdm) is installed, Bundlebase will use it automatically.


## Metrics

Bundlebase includes OpenTelemetry-based metrics.

### Metrics Logging

For development and debugging, Bundlebase provides a simple way to see metrics via stdout without needing Prometheus, Jaeger, or other external collectors:

```python
from bundlebase import log_metrics

logging.basicConfig(
    stream=sys.stdout,
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logging.getLogger("bundlebase").setLevel(logging.DEBUG)

log_metrics()

## Rest of your python code
```

This is perfect for:
- Local development
- Debugging index performance
- Quick performance checks
- Testing without infrastructure setup
