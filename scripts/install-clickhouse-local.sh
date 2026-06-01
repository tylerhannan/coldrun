#!/bin/bash
# Install the latest ClickHouse master binary into clickhouse-local/ (official curl | sh flow).
#
# Usage:
#   ./scripts/install-clickhouse-local.sh
#
# Uses https://clickhouse.com/ (builds.clickhouse.com/master/...) — same as piping curl to sh.
# Set CLICKHOUSE_ONLY=1 to skip clickhousectl side-install.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/clickhouse-local"
mkdir -p "$DIR"

(
  cd "$DIR"
  rm -f clickhouse
  CLICKHOUSE_ONLY=1 curl -fsSL https://clickhouse.com/ | sh
  chmod +x clickhouse
  ./clickhouse --version | tee VERSION
)

echo "Installed: $DIR/clickhouse ($(cat "$DIR/VERSION"))"
