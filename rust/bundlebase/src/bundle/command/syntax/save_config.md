Save a configuration value to the bundle manifest so it persists across sessions. Use FOR to specify the configuration scope.

### Examples

    SAVE CONFIG region = 'us-west-2' FOR 's3'
    SAVE CONFIG endpoint = 'https://s3.example.com' FOR 's3/my-bucket'
