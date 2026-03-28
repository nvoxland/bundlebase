#!/bin/bash
# Wrapper script to launch the Java benchmark function via IPC protocol.
# Finds the correct Java runtime and SDK jar automatically.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SDK_DIR="$SCRIPT_DIR/../../../../sdk/java"
JAR="$SDK_DIR/target/bundlebase-sdk-0.1.0-jar-with-dependencies.jar"

# Find Java 22+ (needed for the SDK)
if [ -x "/opt/homebrew/opt/openjdk/bin/java" ]; then
    JAVA="/opt/homebrew/opt/openjdk/bin/java"
elif command -v java >/dev/null 2>&1; then
    JAVA="java"
else
    echo '{"error": "Java not found"}' >&2
    exit 1
fi

exec "$JAVA" \
    --add-opens java.base/java.nio=ALL-UNNAMED \
    --enable-native-access=ALL-UNNAMED \
    -cp "$JAR:$SCRIPT_DIR" \
    DoubleVal "$@"
