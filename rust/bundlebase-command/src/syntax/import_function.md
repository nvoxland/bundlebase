Import a persistent user-defined SQL function from an external source. The function is saved to the bundle and available across sessions. Use wildcard syntax to import all functions from a library.

### Examples

    IMPORT FUNCTION acme.double_val FROM 'ipc::./my_func'
    IMPORT FUNCTION acme.* FROM 'ffi::./mylib.so'
    IMPORT FUNCTION acme.double_val FROM 'ipc::./my_func' WITH (platform = 'linux/amd64')
