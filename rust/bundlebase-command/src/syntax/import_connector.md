Import a persistent data connector from an external source. The connector is saved to the bundle and available across sessions.

For fat connectors that ship binaries for several OS/arch targets, use the platform map form (`FROM { ... }`) or a `{os}/{arch}/{ext}` glob pattern. Each platform becomes a separate registry entry; at fetch time, the entry whose platform matches the host wins.

Optionally attach a source archive with `WITH (src = '...')`. The archive is copied into the bundle (content-addressed). Recipients can extract it later with `EXPORT SOURCE`.

### Examples

    -- Single platform (or any platform)
    IMPORT CONNECTOR acme.weather FROM 'ipc::./my_source'
    IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so' WITH (platform = 'linux/amd64')

    -- Multi-platform map: one entry per target
    IMPORT CONNECTOR acme.weather FROM {
        'linux/amd64'   : 'ffi::./weather-linux-amd64.so',
        'linux/arm64'   : 'ffi::./weather-linux-arm64.so',
        'darwin/arm64'  : 'ffi::./weather-darwin-arm64.dylib',
        'windows/amd64' : 'ffi::./weather-windows-amd64.dll'
    }

    -- Glob form: scan a directory for matching files
    IMPORT CONNECTOR acme.weather FROM 'ffi::./weather-{os}-{arch}.{ext}'

    -- With a bundled source archive
    IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so'
        WITH (platform = 'linux/amd64', src = './weather-source.zip')

Placeholders in the glob form: `{os}` (linux/darwin/windows), `{arch}` (amd64/arm64/...), `{ext}` (so/dylib/dll, validated against `{os}` if both are present).

The map and glob forms cannot be combined with `WITH (platform = ...)` — the platform comes from the map key or the captured filename. `WITH (src = ...)` works with all three forms; multi-platform IMPORT shares one source archive across every entry.
