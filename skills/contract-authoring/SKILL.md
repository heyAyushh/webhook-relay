---
name: contract-authoring
description: "Write, validate, and debug hook contract.toml files covering the full schema (app, policies, serve, smash, profiles, transports), all supported driver config keys, validation rules, plugin system, and error codes. Use when creating a new app contract, extending an existing one, diagnosing validation failures, configuring hook routing, or troubleshooting toml config errors."
---

# Contract Authoring

`contract.toml` is the runtime configuration for a hook app. It declares which ingress adapters receive events (`serve`), which egress adapters deliver events (`smash`), how events route between them (`routes`), and which combination is active (`profiles`). The validator uses `deny_unknown_fields` — any typo in a key is a hard error.

## Validate-Fix Workflow

1. Write or edit `contract.toml`
2. Run `hook debug capabilities` to validate
3. If an error code appears, consult the error table below → fix the issue → re-validate
4. Only deploy when validation passes clean

---

## Top-Level Structure

```toml
[app]           # required
[policies]      # optional, has safe defaults
[serve]         # required
[smash]         # required
[profiles.*]    # required — at least one profile
[transports.*]  # optional — only needed for mcp_tool_output
```

### `[app]`

```toml
[app]
id = "my-app"           # required, string
name = "My App"         # required, string
version = "1.0.0"       # required, string
description = "..."     # optional
```

### `[policies]`

```toml
[policies]
validation_mode = "strict"   # "strict" (default) | "debug"
allow_no_output = false      # default false — set true to allow profiles with no smash output
no_output_sink = "dlq"       # required when allow_no_output=true: "discard" | "dlq"
```

`debug` mode relaxes non-security checks (empty profile labels) but still enforces all security-critical validation.

---

## `[serve]` — Ingress

```toml
[serve]

[[serve.ingress_adapters]]
id = "..."      # unique id, referenced by profiles
driver = "..."  # see drivers below
# driver-specific keys follow

[[serve.routes]]
id = "..."
source_match = "*"           # glob or literal source name (e.g. "github")
event_type_pattern = "*"     # glob or literal event type (e.g. "pull_request.*")
target_topic = "webhooks.core"
```

### Ingress Drivers

#### `http_webhook_ingress`
```toml
[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"
bind = "0.0.0.0:8080"               # required
path_template = "/webhook/{source}"  # optional
plugins = [...]                      # optional
```

#### `websocket_ingress`
```toml
[[serve.ingress_adapters]]
id = "ws-ingress"
driver = "websocket_ingress"
auth_mode = "..."        # required
path_template = "..."    # optional
plugins = [...]          # optional
```

#### `mcp_ingest_exposed`
```toml
[[serve.ingress_adapters]]
id = "mcp-ingress"
driver = "mcp_ingest_exposed"
transport_driver = "..."       # required
bind = "..."                   # required
auth_mode = "..."              # required
max_payload_bytes = 65536      # required
tool_name = "..."              # optional
path = "..."                   # optional
token_env = "..."              # optional
plugins = [...]                # optional
```

#### `kafka_ingress`
```toml
[[serve.ingress_adapters]]
id = "kafka-ingress"
driver = "kafka_ingress"
topics = "external.topic"     # required
group_id = "my-consumer"      # required
brokers = "..."                # optional (falls back to KAFKA_BROKERS env)
plugins = [...]                # optional
```

---

## `[smash]` — Egress

```toml
[smash]

[[smash.egress_adapters]]
id = "..."
driver = "..."
# driver-specific keys follow

[[smash.routes]]
id = "..."
source_topic_pattern = "webhooks.core"   # glob matched against the Kafka topic
event_filters = ["pull_request.*"]       # optional — filter by event type
destinations = [
  { adapter_id = "my-adapter", required = true },  # required defaults to true
]
```

A route destination with `required = true` (the default) means delivery failure is fatal. Set `required = false` for best-effort side-channel outputs.

### Egress Drivers

#### `openclaw_http_output`
```toml
[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"
url = "http://127.0.0.1:18789/hooks/agent"   # required
token_env = "OPENCLAW_WEBHOOK_TOKEN"          # required — env var name holding the token
timeout_seconds = 20                          # required
max_retries = 5                               # required
plugins = [...]                              # optional
```

#### `mcp_tool_output`
```toml
[[smash.egress_adapters]]
id = "mcp-output"
driver = "mcp_tool_output"
tool_name = "emit_event"    # required
transport_ref = "main"      # required — must match a [transports.*] key
plugins = [...]             # optional
```

#### `websocket_client_output`
```toml
[[smash.egress_adapters]]
id = "ws-client"
driver = "websocket_client_output"
url = "wss://example.com/events"   # required
auth_mode = "..."                  # required
send_timeout_ms = 5000             # required
retry_policy = "..."               # required
plugins = [...]                    # optional
```

#### `websocket_server_output`
```toml
[[smash.egress_adapters]]
id = "ws-server"
driver = "websocket_server_output"
bind = "0.0.0.0:9000"             # required
path = "/events"                   # required
auth_mode = "..."                  # required
max_clients = 100                  # required
queue_depth_per_client = 256       # required
send_timeout_ms = 5000             # required
plugins = [...]                    # optional
```

#### `kafka_output`
```toml
[[smash.egress_adapters]]
id = "kafka-out"
driver = "kafka_output"
topic = "external.output"   # required
key_mode = "..."            # required
plugins = [...]             # optional
```

---

## `[transports.*]` — MCP Transports

Only required when using `mcp_tool_output`. The key becomes the `transport_ref` value.

#### `stdio_jsonrpc`
```toml
[transports.main]
driver = "stdio_jsonrpc"
command = "..."     # optional
args = [...]        # optional
env = { ... }       # optional
```

#### `http_sse`
```toml
[transports.main]
driver = "http_sse"
url = "http://..."   # required
auth_mode = "..."    # required
```

---

## `[profiles.*]`

```toml
[profiles.my-profile]
label = "My Profile"                         # required (non-empty in strict mode)
serve_adapters = ["http-ingress"]            # adapter IDs to activate
smash_adapters = ["openclaw-output"]
serve_routes = ["all-to-core"]              # route IDs to activate
smash_routes = ["core-to-openclaw"]
env = { KEY = "value" }                      # optional env overrides for this profile
```

All IDs in a profile must reference adapters/routes declared in `[serve]` / `[smash]`. Missing references are security-critical errors.

### Profile Activation

Hook CLI picks the profile matching the `--app` id or `--profile` flag:

```bash
hook serve --app default-openclaw          # uses profile named "default-openclaw"
hook smash --app default-openclaw
```

---

## Plugin System

Plugins run in declaration order within an adapter's `plugins = [...]` list.

```toml
plugins = [
  { driver = "event_type_alias", from = "push", to = "git.push" },
  { driver = "require_payload_field", pointer = "/repository/id" },
  { driver = "add_meta_flag", flag = "has-repo" },
]
```

| Driver | Effect |
|---|---|
| `event_type_alias` | Remap event type string |
| `require_payload_field` | Fail closed if JSON pointer is missing |
| `add_meta_flag` | Write a deduplicated flag to envelope metadata |

`require_payload_field` uses JSON Pointer syntax (`/field/nested`). A missing pointer causes the message to be rejected.

---

## Contract Discovery Order

When running `hook serve` or `hook smash`:
1. `--contract <path>` — explicit path
2. `--app <id>` → `apps/<id>/contract.toml`
3. `./contract.toml`
4. Embedded `default-openclaw` fallback

---

## Validation Rules and Error Codes

| Code | Cause |
|---|---|
| `missing_profile` | Profile name not found in contract |
| `missing_serve_adapter` | Profile references undefined serve adapter |
| `missing_smash_adapter` | Profile references undefined smash adapter |
| `missing_serve_route` | Profile references undefined serve route |
| `missing_smash_route` | Profile references undefined smash route |
| `unsupported_ingress_driver` | Active adapter uses unknown driver |
| `unsupported_egress_driver` | Active adapter uses unknown driver |
| `unsupported_transport_driver` | Active transport uses unknown driver |
| `unknown_adapter_key` | Adapter config has an unrecognised key |
| `missing_required_adapter_key` | Required adapter key is absent |
| `empty_required_adapter_value` | Required key exists but is empty/whitespace |
| `missing_transport_ref` | `mcp_tool_output` transport_ref points to missing transport |
| `empty_smash_route_destinations` | Smash route has no destinations |
| `inactive_destination_adapter` | Route destination not active in profile |
| `no_smash_outputs` | Profile has no active smash outputs and `allow_no_output=false` |
| `missing_no_output_sink` | `allow_no_output=true` but `no_output_sink` not set |
| `dlq_without_routes` | `no_output_sink=dlq` but no active smash routes |
| `empty_profile_label` | Profile label is empty (strict mode only) |

**Unknown drivers in inactive adapters are allowed** — you can declare adapters for future profiles without breaking current validation.

---

## Minimal Working Example

```toml
[app]
id = "my-app"
name = "My App"
version = "1.0.0"

[serve]

[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"
bind = "0.0.0.0:8080"

[[serve.routes]]
id = "all-to-core"
source_match = "*"
event_type_pattern = "*"
target_topic = "webhooks.core"

[smash]

[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"
url = "http://127.0.0.1:18789/hooks/agent"
token_env = "OPENCLAW_WEBHOOK_TOKEN"
timeout_seconds = 20
max_retries = 5

[[smash.routes]]
id = "core-to-openclaw"
source_topic_pattern = "webhooks.core"
destinations = [{ adapter_id = "openclaw-output" }]

[profiles.my-app]
label = "My App"
serve_adapters = ["http-ingress"]
smash_adapters = ["openclaw-output"]
serve_routes = ["all-to-core"]
smash_routes = ["core-to-openclaw"]
```

Validate before running:

```bash
hook debug capabilities
hook serve --app my-app --dry-run   # if supported
```
