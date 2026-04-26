Copy the source archive bundled with a connector (via `IMPORT CONNECTOR ... WITH (src = '...')`) to a file on disk. Lets a recipient of a bundle audit, fork, or rebuild the connector.

### Examples

    EXPORT SOURCE acme.weather TO '/tmp/weather-source.zip'
    EXPORT SOURCE acme.weather TO 'connector-source.zip'

The output path may be absolute or relative to the current working directory. Parent directories are created if missing. Errors if the connector is not registered or was imported without a `src` attribute.
