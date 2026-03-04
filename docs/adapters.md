# Adapter Reference

Adapters are the pluggable I/O units in hook. Serve uses **ingress adapters** to receive events; smash uses **egress adapters** to deliver them. Both sides support an optional `plugins` list per adapter.

All adapters are declared in `contract.toml` and activated by naming them in a profile. Adapters not named in the active profile are parsed but never validated for runtime correctness — this lets you declare future adapters without breaking today's startup.

---

## Ingress Adapters (Serve)

### `http_webhook_ingress`

Receives webhook POST requests over HTTP. The path `{source}` segment determines which source handler validates and processes the request.

```toml
[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"
bind = "0.0.0.0:8080"                # required — TCP address to listen on
path_template = "/webhook/{source}"  # optional — default: /webhook/{source}
plugins = [...]                      # optional
```

| Key | Required | Description |
|---|---|---|
| `bind` | yes | TCP address. e.g. `0.0.0.0:8080` or `127.0.0.1:9000`. |
| `path_template` | no | URL path template. `{source}` is replaced by the source name at request time. Default: `/webhook/{source}`. |
| `plugins` | no | Plugin list (see [plugins.md](plugins.md)). |

**Request flow:**
1. POST arrives at `<bind><path_template>` with `{source}` filled.
2. Source handler for `{source}` validates the HMAC signature.
3. Body is parsed as JSON, sanitized, and wrapped into an `EventEnvelope`.
4. Envelope published to `webhooks.<source>`.

**Health endpoints** (always available when http_webhook_ingress is running):
- `GET /health` — liveness (always 200)
- `GET /ready` — readiness including Kafka producer state

---

### `websocket_ingress`

Accepts authenticated WebSocket connections. Each valid text frame containing a JSON object is treated as one inbound event.

```toml
[[serve.ingress_adapters]]
id = "ws-ingress"
driver = "websocket_ingress"
auth_mode = "bearer"                  # required
path_template = "/ingest/ws/{source}" # optional
plugins = [...]                       # optional
```

| Key | Required | Description |
|---|---|---|
| `auth_mode` | yes | Authentication strategy. e.g. `bearer`. |
| `path_template` | no | WebSocket upgrade path. Default: `/ingest/ws/{source}`. |
| `plugins` | no | Plugin list. |

---

### `mcp_ingest_exposed`

Hosts an MCP tool endpoint that external MCP clients call to inject events into serve. This adapter operates in **server mode** — it listens for incoming MCP client connections, it does not connect to any external MCP server.

```toml
[[serve.ingress_adapters]]
id = "mcp-ingress"
driver = "mcp_ingest_exposed"
transport_driver = "http_sse"          # required — must be "http_sse" for exposed ingress
bind = "0.0.0.0:4000"                 # required — address to listen on
auth_mode = "bearer"                  # required — inbound client auth
max_payload_bytes = 65536             # required — max tool call payload size
tool_name = "serve_ingest_event"      # optional — MCP tool name exposed to clients
path = "/mcp"                         # optional — HTTP path for the SSE endpoint
token_env = "MCP_INGEST_TOKEN"        # optional — env var holding the bearer token
plugins = [...]                       # optional
```

| Key | Required | Description |
|---|---|---|
| `transport_driver` | yes | Must be `http_sse` for exposed ingress. |
| `bind` | yes | TCP address to listen on. |
| `auth_mode` | yes | Auth strategy for inbound MCP clients. |
| `max_payload_bytes` | yes | Maximum tool call payload size in bytes. Must be positive. |
| `tool_name` | no | Name of the MCP tool exposed to clients. |
| `path` | no | HTTP path for the SSE endpoint. |
| `token_env` | no | Env var name holding the expected bearer token. |
| `plugins` | no | Plugin list. |

**Tool request schema** (fields sent by MCP clients):
- `source: string` — required
- `payload: object` — required
- `event_type: string` — optional
- `headers: object` — optional
- `metadata: object` — optional

**Tool response schema**:
- `status: string`
- `event_id: string`
- `source: string`
- `event_type: string`
- `kafka_topic: string`
- `queued_at: string`

---

### `kafka_ingress`

Consumes messages from an external Kafka cluster or topic as the ingestion source. Each consumed message is converted to an `EventEnvelope`.

```toml
[[serve.ingress_adapters]]
id = "kafka-ingress"
driver = "kafka_ingress"
topics = ["external.events"]       # required — list of topics to consume
group_id = "hook-kafka-ingress"    # required — consumer group ID
brokers = "external:9092"          # optional — overrides KAFKA_BROKERS
plugins = [...]                    # optional
```

| Key | Required | Description |
|---|---|---|
| `topics` | yes | List of Kafka topics to consume from. Cannot be empty. |
| `group_id` | yes | Consumer group ID. |
| `brokers` | no | Override broker list for this adapter. Falls back to `KAFKA_BROKERS` env var. |
| `plugins` | no | Plugin list. |

---

## Egress Adapters (Smash)

### `openclaw_http_output`

Delivers events to an OpenClaw hook endpoint via HTTP POST. This is the default egress in the `default-openclaw` profile.

```toml
[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"
url = "http://127.0.0.1:18789/hooks/agent"  # required
token_env = "OPENCLAW_WEBHOOK_TOKEN"         # required — env var name holding the token
timeout_seconds = 20                         # required
max_retries = 5                              # required
plugins = [...]                             # optional
```

| Key | Required | Description |
|---|---|---|
| `url` | yes | Full URL of the OpenClaw hook endpoint. |
| `token_env` | yes | Name of the env var holding the bearer token. The value is read at runtime, never stored in the contract. |
| `timeout_seconds` | yes | Per-request timeout. |
| `max_retries` | yes | Number of retry attempts on failure before DLQ. |
| `plugins` | no | Plugin list. |

---

### `mcp_tool_output`

Delivers events by calling an MCP tool on an external MCP server. Requires a named transport in `[transports.*]`.

```toml
[[smash.egress_adapters]]
id = "mcp-output"
driver = "mcp_tool_output"
tool_name = "emit_event"     # required — tool name to call on the MCP server
transport_ref = "main"       # required — key in [transports.*]
plugins = [...]              # optional

[transports.main]
driver = "http_sse"
url = "https://mcp.example.com/sse"
auth_mode = "bearer"
```

| Key | Required | Description |
|---|---|---|
| `tool_name` | yes | MCP tool name to invoke. |
| `transport_ref` | yes | Must match a key in `[transports.*]`. |
| `plugins` | no | Plugin list. |

**Transport drivers:**

`http_sse`:
```toml
[transports.main]
driver = "http_sse"
url = "https://mcp.example.com/sse"   # required
auth_mode = "bearer"                  # required
```

`stdio_jsonrpc`:
```toml
[transports.main]
driver = "stdio_jsonrpc"
command = "/usr/local/bin/my-mcp"   # optional
args = ["--mode", "server"]         # optional
env = { KEY = "value" }             # optional
```

---

### `websocket_client_output`

Connects to an external WebSocket server and sends each event as a JSON text frame.

```toml
[[smash.egress_adapters]]
id = "ws-client"
driver = "websocket_client_output"
url = "wss://events.example.com/stream"  # required
auth_mode = "bearer"                     # required
send_timeout_ms = 5000                   # required
retry_policy = "exponential"             # required
plugins = [...]                          # optional
```

| Key | Required | Description |
|---|---|---|
| `url` | yes | WebSocket server URL. |
| `auth_mode` | yes | Authentication strategy. |
| `send_timeout_ms` | yes | Send timeout per frame in milliseconds. |
| `retry_policy` | yes | Retry strategy on send failure. |
| `plugins` | no | Plugin list. |

---

### `websocket_server_output`

Hosts a WebSocket server and broadcasts each event to all connected clients.

```toml
[[smash.egress_adapters]]
id = "ws-server"
driver = "websocket_server_output"
bind = "0.0.0.0:9000"           # required
path = "/events"                 # required
auth_mode = "bearer"             # required
max_clients = 100                # required
queue_depth_per_client = 256     # required — per-client send queue depth
send_timeout_ms = 5000           # required
plugins = [...]                  # optional
```

| Key | Required | Description |
|---|---|---|
| `bind` | yes | TCP address to listen on. |
| `path` | yes | WebSocket upgrade path. |
| `auth_mode` | yes | Auth strategy for connecting clients. |
| `max_clients` | yes | Maximum concurrent client connections. |
| `queue_depth_per_client` | yes | Per-client outbound queue depth. Messages are dropped if the queue is full and the client is slow. |
| `send_timeout_ms` | yes | Per-client send timeout. |
| `plugins` | no | Plugin list. |

---

### `kafka_output`

Republishes each event as a message to an external Kafka topic.

```toml
[[smash.egress_adapters]]
id = "kafka-out"
driver = "kafka_output"
topic = "external.output.events"  # required
key_mode = "event_id"             # required
plugins = [...]                   # optional
```

| Key | Required | Description |
|---|---|---|
| `topic` | yes | Target Kafka topic. |
| `key_mode` | yes | How to compute the Kafka message key. |
| `plugins` | no | Plugin list. |

---

## Multiple Active Adapters

Both sides support multiple adapters active simultaneously in any combination. A single event can be delivered to multiple destinations via multiple destinations in a smash route:

```toml
[[smash.routes]]
id = "fan-out"
source_topic_pattern = "webhooks.core"
destinations = [
  { adapter_id = "openclaw-output", required = true },
  { adapter_id = "kafka-out",       required = false },
  { adapter_id = "ws-server",       required = false },
]
```

`required = true` (default) — commit is blocked until this delivery succeeds.
`required = false` — failure is logged but never blocks commit or triggers DLQ.

---

## Adapter Validation Rules

- All keys on active adapters are validated at startup. Unknown keys are hard errors.
- Required keys must be present and non-empty. Empty strings are treated as missing.
- Adapters in inactive profiles are parsed but not validated for required keys.
- `mcp_tool_output` requires `transport_ref` to point to an existing `[transports.*]` entry in the same contract.
