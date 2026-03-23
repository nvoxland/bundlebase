Remove a session-only connector from the current session. Optionally drop only a specific platform entrypoint.

### Examples

    DROP TEMP CONNECTOR acme.weather
    DROP TEMP CONNECTOR acme.weather FOR PLATFORM 'linux/amd64'
