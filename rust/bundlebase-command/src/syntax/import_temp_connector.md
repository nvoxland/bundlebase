Import a session-only connector that is not persisted to the bundle. Supports the Python runtime for inline connector definitions.

### Examples

    IMPORT TEMP CONNECTOR acme.weather FROM 'python::mod:Class'
    IMPORT TEMP CONNECTOR acme.weather FROM 'ipc::./my_source' WITH (platform = 'linux/amd64')
