#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_RELAY_PORT=9000
readonly DEFAULT_MOCK_OPENCLAW_PORT=3900
readonly STARTUP_RETRY_COUNT=80
readonly STARTUP_RETRY_DELAY_SECONDS=0.25
readonly VERIFY_RETRY_COUNT=120
readonly VERIFY_RETRY_DELAY_SECONDS=0.25
readonly DEFAULT_RELAY_BIN="target/release/webhook-relay"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly REPO_ROOT

RELAY_PORT="${DEFAULT_RELAY_PORT}"
MOCK_OPENCLAW_PORT="${DEFAULT_MOCK_OPENCLAW_PORT}"
USE_LIVE_OPENCLAW=0
RELAY_BIN="${DEFAULT_RELAY_BIN}"

TMP_DIR=""
RELAY_PID=""
MOCK_PID=""
RELAY_LOG_FILE=""
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
  cat <<'EOF_USAGE' >&2
Usage: scripts/smoke-test-rust.sh [-p relay_port] [-o mock_openclaw_port] [-b relay_binary] [-l]

Options:
  -p  Relay port (default: 9000)
  -o  Mock OpenClaw port (default: 3900)
  -b  Relay binary path (default: target/release/webhook-relay)
  -l  Use live OpenClaw at $OPENCLAW_GATEWAY_URL instead of local mock
EOF_USAGE
}

cleanup() {
  if [ -n "${RELAY_PID}" ] && kill -0 "${RELAY_PID}" 2>/dev/null; then
    kill "${RELAY_PID}" 2>/dev/null || true
    wait "${RELAY_PID}" 2>/dev/null || true
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
  while getopts ":p:o:b:lh" opt; do
    case "${opt}" in
      p) RELAY_PORT="${OPTARG}" ;;
      o) MOCK_OPENCLAW_PORT="${OPTARG}" ;;
      b) RELAY_BIN="${OPTARG}" ;;
      l) USE_LIVE_OPENCLAW=1 ;;
      h) usage; exit 0 ;;
      :) die "missing value for -${OPTARG}" ;;
      \?) usage; die "unknown option: -${OPTARG}" ;;
    esac
  done
}

wait_for_http_ok() {
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

wait_for_forwarded_events() {
  local expected_count="$1"
  local attempt=0

  while [ "${attempt}" -lt "${VERIFY_RETRY_COUNT}" ]; do
    local count
    count="$(wc -l < "${MOCK_LOG_FILE}" | tr -d '[:space:]')"
    if [ "${count}" = "${expected_count}" ]; then
      return 0
    fi
    attempt=$((attempt + 1))
    sleep "${VERIFY_RETRY_DELAY_SECONDS}"
  done

  return 1
}

wait_for_expected_metrics() {
  local metrics_url="$1"
  local attempt=0

  while [ "${attempt}" -lt "${VERIFY_RETRY_COUNT}" ]; do
    if python3 - "${metrics_url}" <<'PY'; then
import re
import sys
from urllib.request import urlopen

if len(sys.argv) != 2:
    raise SystemExit(2)

url = sys.argv[1]
with urlopen(url, timeout=2) as response:
    payload = response.read().decode("utf-8")

line_pattern = re.compile(
    r'^([a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{([^}]*)\})?\s+([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)$'
)
label_pattern = re.compile(r'([a-zA-Z_][a-zA-Z0-9_]*)="((?:\\.|[^"])*)"')

def parse_labels(raw: str):
    if not raw:
        return tuple()
    labels = {}
    pos = 0
    for match in label_pattern.finditer(raw):
        if match.start() != pos:
            return None
        labels[match.group(1)] = match.group(2)
        pos = match.end()
        if pos < len(raw):
            if raw[pos] != ',':
                return None
            pos += 1
    if pos != len(raw):
        return None
    return tuple(sorted(labels.items()))

metrics = {}
for raw_line in payload.splitlines():
    line = raw_line.strip()
    if not line or line.startswith("#"):
        continue
    match = line_pattern.match(line)
    if not match:
        continue
    labels = parse_labels(match.group(2) or "")
    if labels is None:
        continue
    metrics[(match.group(1), labels)] = float(match.group(3))


def expect(name: str, labels: dict[str, str], expected: float) -> bool:
    key = (name, tuple(sorted(labels.items())))
    return metrics.get(key, 0.0) == expected

ok = True
ok &= expect("webhook_relay_events_received_total", {"source": "github"}, 2.0)
ok &= expect("webhook_relay_events_forwarded_total", {"source": "github"}, 1.0)
ok &= expect("webhook_relay_events_dropped_total", {"source": "github", "reason": "duplicate_delivery"}, 1.0)
ok &= expect("webhook_relay_events_received_total", {"source": "linear"}, 2.0)
ok &= expect("webhook_relay_events_forwarded_total", {"source": "linear"}, 1.0)
ok &= expect("webhook_relay_events_dropped_total", {"source": "linear", "reason": "duplicate_delivery"}, 1.0)

raise SystemExit(0 if ok else 1)
PY
      return 0
    fi

    attempt=$((attempt + 1))
    sleep "${VERIFY_RETRY_DELAY_SECONDS}"
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
    def log_message(self, format, *args):
        return

    def _write_json(self, status, payload):
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

        with open(log_file, "a", encoding="utf-8") as fh:
            fh.write(json.dumps({
                "path": self.path,
                "source": self.headers.get("X-Webhook-Source", ""),
            }) + "\n")

        self._write_json(202, {"accepted": True})

server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY

  MOCK_PID="$!"
  ACTIVE_OPENCLAW_GATEWAY_URL="http://127.0.0.1:${MOCK_OPENCLAW_PORT}"

  wait_for_http_ok "${ACTIVE_OPENCLAW_GATEWAY_URL}/health" \
    || die "mock OpenClaw did not start on port ${MOCK_OPENCLAW_PORT}"
}

start_relay() {
  RELAY_LOG_FILE="${TMP_DIR}/relay.log"
  : > "${RELAY_LOG_FILE}"

  if [ ! -x "${RELAY_BIN}" ]; then
    log "relay binary not found at ${RELAY_BIN}; building release binary"
    cargo build --release >/dev/null
  fi

  WEBHOOK_BIND_ADDR="0.0.0.0:${RELAY_PORT}" \
  WEBHOOK_DB_PATH="${TMP_DIR}/relay.redb" \
  OPENCLAW_GATEWAY_URL="${ACTIVE_OPENCLAW_GATEWAY_URL}" \
  "${RELAY_BIN}" >"${RELAY_LOG_FILE}" 2>&1 &
  RELAY_PID="$!"

  wait_for_http_ok "http://127.0.0.1:${RELAY_PORT}/health" \
    || die "relay did not become healthy; see ${RELAY_LOG_FILE}"
}

send_github_event() {
  local delivery_id="$1"
  local body
  local signature
  local status

  body='{"action":"opened","pull_request":{"number":42,"title":"fix: null guard","body":"Adds a null check before dereference.","draft":false,"merged":false,"state":"open","head":{"ref":"fix/null-guard","sha":"abc123"},"base":{"ref":"main","sha":"def456"},"user":{"login":"dev"},"changed_files":2,"additions":10,"deletions":3},"repository":{"full_name":"org/repo","default_branch":"main"},"sender":{"login":"dev"},"installation":{"id":12345}}'
  signature="$(printf '%s' "${body}" | openssl dgst -sha256 -hmac "${GITHUB_WEBHOOK_SECRET}" | awk '{print $NF}')"

  status="$(curl --silent --output /dev/null --write-out "%{http_code}" \
    -X POST "http://127.0.0.1:${RELAY_PORT}/hooks/github-pr" \
    -H "Content-Type: application/json" \
    -H "X-GitHub-Event: pull_request" \
    -H "X-GitHub-Delivery: ${delivery_id}" \
    -H "X-Hub-Signature-256: sha256=${signature}" \
    -d "${body}")"

  case "${status}" in
    2*) ;;
    *) die "GitHub request failed with HTTP status: ${status}" ;;
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
    -X POST "http://127.0.0.1:${RELAY_PORT}/hooks/linear" \
    -H "Content-Type: application/json" \
    -H "Linear-Signature: ${signature}" \
    -H "Linear-Delivery: ${delivery_id}" \
    -d "${body}")"

  case "${status}" in
    2*) ;;
    *) die "Linear request failed with HTTP status: ${status}" ;;
  esac
}

verify_mock_openclaw_observed_events() {
  python3 - "${MOCK_LOG_FILE}" <<'PY'
import json
import sys
from urllib.parse import parse_qs, urlparse

if len(sys.argv) != 2:
    raise SystemExit(2)

records = []
with open(sys.argv[1], encoding="utf-8") as fh:
    for line in fh:
        line = line.strip()
        if line:
            records.append(json.loads(line))

if len(records) != 2:
    print(f"expected exactly 2 forwarded events, got {len(records)}", file=sys.stderr)
    raise SystemExit(1)

sources = set()
for record in records:
    query = parse_qs(urlparse(record["path"]).query)
    sources.add(query.get("source", [""])[0])

if sources != {"github-pr", "linear"}:
    print(f"unexpected forwarded sources: {sorted(sources)}", file=sys.stderr)
    raise SystemExit(1)
PY
}

main() {
  parse_args "$@"

  require_cmd "curl"
  require_cmd "openssl"
  require_cmd "python3"

  require_env "GITHUB_WEBHOOK_SECRET"
  require_env "LINEAR_WEBHOOK_SECRET"
  require_env "OPENCLAW_HOOKS_TOKEN"

  TMP_DIR="$(mktemp -d)"

  if [ "${USE_LIVE_OPENCLAW}" -eq 1 ]; then
    require_env "OPENCLAW_GATEWAY_URL"
    ACTIVE_OPENCLAW_GATEWAY_URL="${OPENCLAW_GATEWAY_URL}"
    MOCK_LOG_FILE=""
  else
    start_mock_openclaw
  fi

  start_relay

  send_github_event "smoke-gh-1"
  send_linear_event "smoke-linear-1"

  send_github_event "smoke-gh-1"
  send_linear_event "smoke-linear-1"

  if [ "${USE_LIVE_OPENCLAW}" -eq 0 ]; then
    wait_for_forwarded_events "2" || die "timed out waiting for forwarded events"
    verify_mock_openclaw_observed_events
    wait_for_expected_metrics "http://127.0.0.1:${RELAY_PORT}/metrics" || die "metrics did not reach expected values"
    log "rust smoke test passed: forwarded one GitHub + one Linear event; duplicates dropped"
  else
    log "rust smoke test sent signed events to live OpenClaw at ${ACTIVE_OPENCLAW_GATEWAY_URL}"
  fi

  log "relay log: ${RELAY_LOG_FILE}"
  if [ -n "${MOCK_LOG_FILE:-}" ]; then
    log "mock log: ${MOCK_LOG_FILE}"
  fi
}

main "$@"
