Set a runtime configuration value for the current session only. The value is not persisted to the bundle manifest. Use FOR to specify the configuration scope.

### Examples

    SET CONFIG access_key_id = 'AKIA...' FOR 's3'
    SET CONFIG secret_access_key = 'secret' FOR 's3'
    SET CONFIG region = 'us-west-2' FOR 's3/my-bucket'
