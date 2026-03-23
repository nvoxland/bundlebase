Import a session-only function that is not persisted to the bundle. Supports the Python runtime for inline function definitions. Use wildcard syntax to import all functions from a library.

### Examples

    IMPORT TEMP FUNCTION acme.double_val FROM 'python::mod:func'
    IMPORT TEMP FUNCTION acme.* FROM 'ffi::./mylib.so'
    IMPORT TEMP FUNCTION acme.process FROM 'ipc::./my_func' WITH (platform = 'linux/amd64')
