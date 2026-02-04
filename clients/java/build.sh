#!/bin/bash
# Build the Acteon Java client JAR with dependencies
set -e

cd "$(dirname "$0")"

echo "Building Acteon Java client..."
gradle shadowJar -q

JAR_PATH="build/libs/acteon-client-0.1.0.jar"

if [ -f "$JAR_PATH" ]; then
    echo "✓ Built: $JAR_PATH"
    echo "  Size: $(du -h "$JAR_PATH" | cut -f1)"
else
    echo "✗ Build failed"
    exit 1
fi
