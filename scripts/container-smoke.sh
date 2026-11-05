#!/usr/bin/env bash
# Shared by CI and release acceptance. /healthz tests liveness, without a Tines token.
set -euo pipefail
image="${1:?usage: container-smoke.sh IMAGE}"
test "$(docker image inspect "$image" --format '{{.Config.User}}')" = "10001:10001"
container="tines-mcp-smoke-$$-$RANDOM"
cleanup() {
  docker rm --force "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT
# Let Docker select an unused loopback port; never expose the smoke listener publicly.
docker run --detach --name "$container" --publish 127.0.0.1::8080 --env MCP_BIND_ADDR=0.0.0.0:8080 "$image"
port="$(docker port "$container" 8080/tcp)"
[[ "$port" =~ ^127\.0\.0\.1:[0-9]+$ ]]
status=""
for attempt in $(seq 1 30); do
  status="$(curl --disable --noproxy '*' -sS --max-time 2 -o /dev/null -w '%{http_code}' "http://$port/healthz" || true)"
  if [[ "$status" == "204" ]]; then
    break
  fi
  sleep 1
done
test "$status" = "204"
docker stop --time 60 "$container"
test "$(docker inspect --format '{{.State.ExitCode}}' "$container")" = "0"
