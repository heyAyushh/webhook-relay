#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_WEBHOOK_PORT=9000
readonly DEFAULT_MOCK_OPENCLAW_PORT=3900
readonly STARTUP_RETRY_COUNT=60
readonly STARTUP_RETRY_DELAY_SECONDS=0.2

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly REPO_ROOT

HOOKS_FILE="${REPO_ROOT}/hooks.yaml"
WEBHOOK_PORT="${DEFAULT_WEBHOOK_PORT}"
MOCK_OPENCLAW_PORT="${DEFAULT_MOCK_OPENCLAW_PORT}"
USE_LIVE_OPENCLAW=0

TMP_DIR=""
WEBHOOK_PID=""
MOCK_PID=""
WEBHOOK_LOG_FILE=""
MOCK_LOG_FILE=""
ACTIVE_OPENCLAW_GATEWAY_URL=""

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

usage() {
  cat <<'EOF' >&2
Usage: scripts/smoke-test.sh [-f hooks_file] [-p webhook_port] [-o mock_openclaw_port] [-l]

Options:
  -f  Path to hooks config (default: ./hooks.yaml)
  -p  Local webhook port (default: 9000)
  -o  Mock OpenClaw port (default: 3900)
  -l  Use live OpenClaw at $OPENCLAW_GATEWAY_URL instead of local mock
EOF
}

cleanup() {
  if [ -n "${WEBHOOK_PID}" ] && kill -0 "${WEBHOOK_PID}" 2>/dev/null; then
    kill "${WEBHOOK_PID}" 2>/dev/null || true
    wait "${WEBHOOK_PID}" 2>/dev/null || true
  fi
  if [ -n "${MOCK_PID}" ] && kill -0 "${MOCK_PID}" 2>/dev/null; then
    kill "${MOCK_PID}" 2>/dev/null || true
    wait "${MOCK_PID}" 2>/dev/null || true
  fi
  if [ -n "${TMP_DIR}" ] && [ -d "${TMP_DIR}" ]; then
    rm -rf "${TMP_DIR}"
  fi
}

trap cleanup EXIT INT TERM

require_cmd() {
  local cmd="$1"
  command -v "${cmd}" >/dev/null 2>&1 || die "missing required command: ${cmd}"
}

require_env() {
  local name="$1"
  if [ -z "${!name:-}" ]; then
    die "missing required environment variable: ${name}"
  fi
}

parse_args() {
  while getopts ":f:p:o:lh" opt; do
    case "${opt}" in
      f) HOOKS_FILE="${OPTARG}" ;;
      p) WEBHOOK_PORT="${OPTARG}" ;;
      o) MOCK_OPENCLAW_PORT="${OPTARG}" ;;
      l) USE_LIVE_OPENCLAW=1 ;;
      h) usage; exit 0 ;;
      :) die "missing value for -${OPTARG}" ;;
      \?) usage; die "unknown option: -${OPTARG}" ;;
    esac
  done
}

wait_for_webhook_ready() {
  local url="$1"
  local attempt=0

  while [ "${attempt}" -lt "${STARTUP_RETRY_COUNT}" ]; do
    local status
    status="$(curl --silent --output /dev/null --write-out "%{http_code}" "${url}" || true)"
    if [ "${status}" = "405" ] || [ "${status}" = "400" ] || [ "${status}" = "200" ]; then
      return 0
    fi
    attempt=$((attempt + 1))
    sleep "${STARTUP_RETRY_DELAY_SECONDS}"
  done

  return 1
}

wait_for_health_endpoint() {
  local url="$1"
  local attempt=0

  while [ "${attempt}" -lt "${STARTUP_RETRY_COUNT}" ]; do
    if curl --silent --fail "${url}" >/dev/null 2>&1; then
      return 0
    fi
    attempt=$((attempt + 1))
    sleep "${STARTUP_RETRY_DELAY_SECONDS}"
  done

  return 1
}

start_mock_openclaw() {
  require_env "OPENCLAW_HOOKS_TOKEN"

  MOCK_LOG_FILE="${TMP_DIR}/mock-openclaw.jsonl"
  : > "${MOCK_LOG_FILE}"

  python3 - "${MOCK_OPENCLAW_PORT}" "${OPENCLAW_HOOKS_TOKEN}" "${MOCK_LOG_FILE}" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

if len(sys.argv) != 4:
    raise SystemExit(2)

port = int(sys.argv[1])
token = sys.argv[2]
log_file = sys.argv[3]

class Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):  # noqa: A003
        return

    def _write_json(self, status: int, payload: dict):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(payload).encode("utf-8"))

    def do_GET(self):
        if self.path == "/health":
            self._write_json(200, {"ok": True})
            return
        self._write_json(404, {"error": "not_found"})

    def do_POST(self):
        expected_auth = f"Bearer {token}"
        if self.headers.get("Authorization", "") != expected_auth:
            self._write_json(401, {"error": "unauthorized"})
            return

        content_length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(content_length).decode("utf-8")
        try:
            json.loads(body)
        except json.JSONDecodeError:
            self._write_json(400, {"error": "invalid_json"})
            return

        record = {
            "path": self.path,
            "source": self.headers.get("X-Webhook-Source", ""),
        }
        with open(log_file, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(record) + "\n")

        self._write_json(202, {"accepted": True})

server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY

  MOCK_PID="$!"
  ACTIVE_OPENCLAW_GATEWAY_URL="http://127.0.0.1:${MOCK_OPENCLAW_PORT}"

  wait_for_health_endpoint "${ACTIVE_OPENCLAW_GATEWAY_URL}/health" \
    || die "mock OpenClaw did not start on port ${MOCK_OPENCLAW_PORT}"
}

start_webhook_server() {
  WEBHOOK_LOG_FILE="${TMP_DIR}/webhook.log"
  : > "${WEBHOOK_LOG_FILE}"

  WEBHOOK_DEDUP_DIR="${TMP_DIR}/dedup" \
  OPENCLAW_GATEWAY_URL="${ACTIVE_OPENCLAW_GATEWAY_URL}" \
  webhook -hooks "${HOOKS_FILE}" -verbose -port "${WEBHOOK_PORT}" >"${WEBHOOK_LOG_FILE}" 2>&1 &
  WEBHOOK_PID="$!"

  wait_for_webhook_ready "http://127.0.0.1:${WEBHOOK_PORT}/hooks/github-pr" \
    || die "webhook server did not become ready; see ${WEBHOOK_LOG_FILE}"
}

send_github_event() {
  local delivery_id="$1"
  local body
  local signature
  local status

  body='{"action":"opened","pull_request":{"number":42,"title":"fix: null guard","body":"Adds a null check before dereference.","draft":false,"merged":false,"state":"open","head":{"ref":"fix/null-guard","sha":"abc123"},"base":{"ref":"main","sha":"def456"},"user":{"login":"dev"},"changed_files":2,"additions":10,"deletions":3},"repository":{"full_name":"org/repo","default_branch":"main"},"sender":{"login":"dev"},"installation":{"id":12345}}'
  signature="$(printf '%s' "${body}" | openssl dgst -sha256 -hmac "${GITHUB_WEBHOOK_SECRET}" | awk '{print $NF}')"

  status="$(curl --silent --output /dev/null --write-out "%{http_code}" \
    -X POST "http://127.0.0.1:${WEBHOOK_PORT}/hooks/github-pr" \
    -H "Content-Type: application/json" \
    -H "X-GitHub-Event: pull_request" \
    -H "X-GitHub-Delivery: ${delivery_id}" \
    -H "X-Hub-Signature-256: sha256=${signature}" \
    -d "${body}")"

  case "${status}" in
    2*) ;;
    *) die "GitHub webhook request failed with HTTP status: ${status}" ;;
  esac
}

send_linear_event() {
  local delivery_id="$1"
  local now_ms
  local body
  local signature
  local status

  now_ms="$(( $(date +%s) * 1000 ))"
  body="{\"webhookTimestamp\":${now_ms},\"type\":\"Issue\",\"action\":\"create\",\"url\":\"https://linear.app/org/issue/ENG-42\",\"data\":{\"id\":\"issue-42\",\"identifier\":\"ENG-42\",\"team\":{\"key\":\"ENG\"},\"priority\":2,\"assignee\":{\"name\":\"Dev\"},\"labels\":[{\"name\":\"backend\"}],\"title\":\"Harden webhook relay\",\"description\":\"Validate signatures and dedup deliveries.\",\"userId\":\"user-123\"}}"
  signature="$(printf '%s' "${body}" | openssl dgst -sha256 -hmac "${LINEAR_WEBHOOK_SECRET}" | awk '{print $NF}')"

  status="$(curl --silent --output /dev/null --write-out "%{http_code}" \
    -X POST "http://127.0.0.1:${WEBHOOK_PORT}/hooks/linear" \
    -H "Content-Type: application/json" \
    -H "Linear-Signature: ${signature}" \
    -H "Linear-Delivery: ${delivery_id}" \
    -d "${body}")"

  case "${status}" in
    2*) ;;
    *) die "Linear webhook request failed with HTTP status: ${status}" ;;
  esac
}

verify_mock_openclaw_observed_events() {
  python3 - "${MOCK_LOG_FILE}" <<'PY'
import json
import sys
from urllib.parse import parse_qs, urlparse

if len(sys.argv) != 2:
    raise SystemExit(2)

path = sys.argv[1]
records = []
with open(path, encoding="utf-8") as fh:
    for line in fh:
        line = line.strip()
        if not line:
            continue
        records.append(json.loads(line))

if len(records) != 2:
    print(f"expected exactly 2 forwarded events (dedup applied), got {len(records)}", file=sys.stderr)
    raise SystemExit(1)

sources = set()
for rec in records:
    query = parse_qs(urlparse(rec["path"]).query)
    sources.add(query.get("source", [""])[0])

expected = {"github-pr", "linear"}
if sources != expected:
    print(f"expected sources {sorted(expected)}, got {sorted(sources)}", file=sys.stderr)
    raise SystemExit(1)
PY
}

main() {
  parse_args "$@"

  require_cmd "webhook"
  require_cmd "curl"
  require_cmd "openssl"
  require_cmd "python3"

  [ -f "${HOOKS_FILE}" ] || die "hooks file not found: ${HOOKS_FILE}"

  require_env "GITHUB_WEBHOOK_SECRET"
  require_env "LINEAR_WEBHOOK_SECRET"
  require_env "OPENCLAW_HOOKS_TOKEN"

  TMP_DIR="$(mktemp -d)"

  if [ "${USE_LIVE_OPENCLAW}" -eq 1 ]; then
    require_env "OPENCLAW_GATEWAY_URL"
    ACTIVE_OPENCLAW_GATEWAY_URL="${OPENCLAW_GATEWAY_URL}"
  else
    start_mock_openclaw
  fi

  start_webhook_server

  # Initial deliveries (should forward)
  send_github_event "smoke-gh-1"
  send_linear_event "smoke-linear-1"

  # Duplicate deliveries (should be deduplicated by relay scripts)
  send_github_event "smoke-gh-1"
  send_linear_event "smoke-linear-1"

  if [ "${USE_LIVE_OPENCLAW}" -eq 0 ]; then
    verify_mock_openclaw_observed_events
    log "smoke test passed: forwarded exactly one GitHub + one Linear event (duplicates skipped)"
  else
    log "smoke test sent signed events to live OpenClaw at ${ACTIVE_OPENCLAW_GATEWAY_URL}"
  fi

  log "webhook log: ${WEBHOOK_LOG_FILE}"
  if [ -n "${MOCK_LOG_FILE}" ]; then
    log "mock log: ${MOCK_LOG_FILE}"
  fi
}

main "$@"
