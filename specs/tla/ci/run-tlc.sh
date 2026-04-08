#!/usr/bin/env bash
# -------------------------------------------------------------------
# Run TLC model checker on all TLA+ specs in specs/tla/.
#
# Usage:
#   ./specs/tla/ci/run-tlc.sh              # check all specs
#   ./specs/tla/ci/run-tlc.sh CircuitBreaker  # check one spec
#
# Requirements:
#   - Java 11+ on PATH
#   - tla2tools.jar (downloaded automatically if missing)
#
# Exit codes:
#   0  All specs pass
#   1  One or more specs have violations
#   2  Setup error (Java missing, download failed, etc.)
# -------------------------------------------------------------------
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPECS_DIR="$(dirname "$SCRIPT_DIR")"
TOOLS_DIR="${SPECS_DIR}/.tools"
TLA2TOOLS_JAR="${TOOLS_DIR}/tla2tools.jar"
TLA2TOOLS_URL="https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar"

# Colors (disabled if not a terminal)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    NC='\033[0m'
else
    RED='' GREEN='' YELLOW='' NC=''
fi

# -------------------------------------------------------------------
# Preflight checks
# -------------------------------------------------------------------
if ! command -v java &>/dev/null; then
    echo -e "${RED}Error: Java not found on PATH.${NC}"
    echo "Install Java 11+ or set JAVA_HOME."
    exit 2
fi

# Download tla2tools.jar if not present
if [ ! -f "$TLA2TOOLS_JAR" ]; then
    echo -e "${YELLOW}Downloading tla2tools.jar...${NC}"
    mkdir -p "$TOOLS_DIR"
    if command -v curl &>/dev/null; then
        curl -fsSL -o "$TLA2TOOLS_JAR" "$TLA2TOOLS_URL"
    elif command -v wget &>/dev/null; then
        wget -q -O "$TLA2TOOLS_JAR" "$TLA2TOOLS_URL"
    else
        echo -e "${RED}Error: Neither curl nor wget available.${NC}"
        exit 2
    fi
    echo -e "${GREEN}Downloaded tla2tools.jar${NC}"
fi

# -------------------------------------------------------------------
# Discover specs to check
# -------------------------------------------------------------------
if [ $# -gt 0 ]; then
    SPECS=("$@")
else
    # Find all .cfg files (each corresponds to a spec)
    SPECS=()
    for cfg in "$SPECS_DIR"/*.cfg; do
        [ -f "$cfg" ] || continue
        name="$(basename "$cfg" .cfg)"
        SPECS+=("$name")
    done
fi

if [ ${#SPECS[@]} -eq 0 ]; then
    echo -e "${YELLOW}No specs found in ${SPECS_DIR}${NC}"
    exit 0
fi

# -------------------------------------------------------------------
# Run TLC on each spec
# -------------------------------------------------------------------
FAILED=0
PASSED=0

for spec in "${SPECS[@]}"; do
    tla_file="${SPECS_DIR}/${spec}.tla"
    cfg_file="${SPECS_DIR}/${spec}.cfg"

    if [ ! -f "$tla_file" ]; then
        echo -e "${RED}SKIP ${spec}: ${tla_file} not found${NC}"
        continue
    fi
    if [ ! -f "$cfg_file" ]; then
        echo -e "${RED}SKIP ${spec}: ${cfg_file} not found${NC}"
        continue
    fi

    echo -e "${YELLOW}Checking ${spec}...${NC}"

    # Run TLC with:
    #   -workers auto    Use all available cores
    #   -cleanup         Remove generated files after run
    #   -deadlock        Report deadlocks as errors
    output_dir=$(mktemp -d)
    set +e
    java -XX:+UseParallelGC \
         -jar "$TLA2TOOLS_JAR" \
         -config "$cfg_file" \
         -workers auto \
         -deadlock \
         -metadir "$output_dir" \
         "$tla_file" 2>&1 | tee "${output_dir}/tlc-output.txt"
    rc=${PIPESTATUS[0]}
    set -e

    # TLC exit codes:
    #   0  = no errors
    #   10 = assumption failure
    #   11 = deadlock
    #   12 = safety violation
    #   13 = liveness violation
    if [ $rc -eq 0 ]; then
        echo -e "${GREEN}PASS ${spec}${NC}"
        PASSED=$((PASSED + 1))
    else
        echo -e "${RED}FAIL ${spec} (exit code ${rc})${NC}"
        FAILED=$((FAILED + 1))
    fi

    rm -rf "$output_dir"
    echo ""
done

# -------------------------------------------------------------------
# Summary
# -------------------------------------------------------------------
echo "========================================"
echo -e "Results: ${GREEN}${PASSED} passed${NC}, ${RED}${FAILED} failed${NC}"
echo "========================================"

if [ $FAILED -gt 0 ]; then
    exit 1
fi
