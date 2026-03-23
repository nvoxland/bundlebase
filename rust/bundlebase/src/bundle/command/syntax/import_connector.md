Import a persistent data connector from an external source. The connector is saved to the bundle and available across sessions. Use WITH to specify platform-specific entrypoints.

### Examples

    IMPORT CONNECTOR acme.weather FROM 'ipc::./my_source'
    IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so' WITH (platform = 'linux/amd64')
