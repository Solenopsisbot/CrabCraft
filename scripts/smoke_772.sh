#!/usr/bin/env bash
set -euo pipefail

# Downloads no assets and writes all server state to a temporary directory.
# Requires assets-cache/servers/1.21.8/server.jar and a Java runtime.
root=$(cd "$(dirname "$0")/.." && pwd)
port=${CRABCRAFT_SMOKE_PORT:-25580}
run_dir=$(mktemp -d "${TMPDIR:-/tmp}/crabcraft-772-smoke.XXXXXX")
trap 'kill "$server_pid" 2>/dev/null || true' EXIT
cp "$root/assets-cache/servers/1.21.8/server.jar" "$run_dir/server.jar"
printf 'eula=true\n' > "$run_dir/eula.txt"
printf 'eula=true\nonline-mode=false\nserver-port=%s\n' "$port" > "$run_dir/server.properties"
(cd "$run_dir" && java -Xms512M -Xmx1G -jar server.jar nogui >server.log 2>&1) & server_pid=$!
for _ in $(seq 1 60); do
	grep -q 'Done (' "$run_dir/server.log" && break
	sleep 1
done
grep -q 'Done (' "$run_dir/server.log"
(cd "$root" && CRABCRAFT_PROTOCOL="${CRABCRAFT_PROTOCOL:-772}" cargo run -q -p crabcraft -- "127.0.0.1:$port" SmokeBot "${CRABCRAFT_SMOKE_SECONDS:-10}")
