#!/usr/bin/env bash
# Deploy the tache webhook server on the droplet. Runs as zach (docker
# group), no root needed. Reads from env: IMAGE
set -euo pipefail

: "${IMAGE:?IMAGE required}"

NAME="tache"
PORT=8321
ENV_FILE="/home/zach/tache/.env"
CADDY_SNIPPET="/etc/caddy/conf.d/tache.caddy"

if [[ ! -f "${ENV_FILE}" ]]; then
  echo "!! ${ENV_FILE} missing (needs TODOIST_API_TOKEN, TODOIST_CLIENT_SECRET)" >&2
  exit 1
fi

echo "==> pulling ${IMAGE}"
docker pull "${IMAGE}"

echo "==> replacing container ${NAME} (host port ${PORT})"
docker rm -f "${NAME}" >/dev/null 2>&1 || true
docker run -d \
  --name "${NAME}" \
  --restart unless-stopped \
  -p "127.0.0.1:${PORT}:8321" \
  --env-file "${ENV_FILE}" \
  "${IMAGE}" >/dev/null

echo "==> waiting for ${NAME} to become ready"
deadline=$(( $(date +%s) + 120 ))
ready=0
while [[ $(date +%s) -lt $deadline ]]; do
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 "http://127.0.0.1:${PORT}/healthz" || echo 000)
  if [[ "$code" == "200" ]]; then
    ready=1
    break
  fi
  sleep 2
done

if [[ $ready -ne 1 ]]; then
  echo "==> readiness check timed out for ${NAME}"
  docker inspect "${NAME}" --format '{{.State.Status}} restarts={{.RestartCount}} exit={{.State.ExitCode}}' || true
  docker logs --tail 100 "${NAME}" 2>&1 || true
  exit 1
fi

if [[ ! -f "${CADDY_SNIPPET}" ]]; then
  echo "!! ${CADDY_SNIPPET} missing — run once as root:"
  echo "   printf 'tache.zach.network {\\n\\treverse_proxy 127.0.0.1:${PORT}\\n}\\n' > ${CADDY_SNIPPET} && systemctl reload caddy"
fi

echo "==> deployed: https://tache.zach.network"
