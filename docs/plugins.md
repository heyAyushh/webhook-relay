# Plugin System

Both serve ingress adapters and smash egress adapters support an optional `plugins` list. Plugins run in declaration order and can transform or filter the event before it is published (serve) or delivered (smash).

---

## How Plugins Work

Plugins are declared inline on an adapter:

```toml
[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"
bind = "0.0.0.0:8080"
plugins = [
  { driver = "require_payload_field", pointer = "/repository/id" },
  { driver = "event_type_alias", from = "push", to = "git.push" },
  { driver = "add_meta_flag", flag = "has-repo" },
]
```

Execution rules:
- Plugins run **in declaration order**. Order matters when one plugin affects input to another.
- A plugin failure is **fail-closed** — the event is rejected and the pipeline stops.
- Plugins are validated at startup for active adapters. Invalid plugin config blocks startup.
- Plugin configuration is validated for required/optional keys exactly like adapter config.

---

## Plugin Drivers

### `event_type_alias`

Rewrites the event type string. Useful for normalizing source-specific event names into a canonical vocabulary, or for routing based on a renamed type.

```toml
{ driver = "event_type_alias", from = "push", to = "git.push" }
```

| Key | Required | Description |
|---|---|---|
| `from` | yes | Event type string to match (exact, case-sensitive). Cannot be empty. |
| `to` | yes | Replacement event type string. Cannot be empty. |

The rewrite applies only when the current event type exactly equals `from`. If no match, the plugin is a no-op.

**Example**: Serve receives a GitHub `push` event with `event_type = "push"`. After `event_type_alias`, the envelope carries `event_type = "git.push"`. Smash routes can then filter on `git.push` specifically.

---

### `require_payload_field`

Fails the event if a specific field is absent from the payload. Uses JSON Pointer syntax. Enforces payload structure at the boundary.

```toml
{ driver = "require_payload_field", pointer = "/repository/id" }
```

| Key | Required | Description |
|---|---|---|
| `pointer` | yes | JSON Pointer (RFC 6901) path. Must start with `/`. Cannot be empty. |

If the field pointed to does not exist in the payload, the event is **rejected** — the pipeline stops with an error, no envelope is created, and the source receives an appropriate error response.

**JSON Pointer syntax:**
- `/field` — top-level field
- `/nested/field` — nested field
- `/array/0` — first element of an array
- `/field~1with~1slashes` — field containing `/` (escaped as `~1`)
- `/field~0with~0tildes` — field containing `~` (escaped as `~0`)

**Example**: Require that all GitHub pull_request events include a repository ID:

```toml
{ driver = "require_payload_field", pointer = "/repository/id" }
```

Events missing `payload.repository.id` are rejected before they reach Kafka.

---

### `add_meta_flag`

Adds a string flag to `EventEnvelope.meta.flags`. Flags are deduplicated — adding the same flag twice results in one entry.

```toml
{ driver = "add_meta_flag", flag = "has-repo" }
```

| Key | Required | Description |
|---|---|---|
| `flag` | yes | String flag to append to `meta.flags`. Cannot be empty. |

The flag appears in the serialized envelope as part of `meta.flags`. Downstream consumers (smash adapters, external systems) can use flags to branch on event characteristics without re-parsing the payload.

**Example**: Tag events that have passed a payload field check:

```toml
plugins = [
  { driver = "require_payload_field", pointer = "/repository/id" },
  { driver = "add_meta_flag", flag = "has-repo" },
]
```

Any event that reaches the `add_meta_flag` step has already passed `require_payload_field`, so the flag reliably signals that the field was present.

---

## Plugin Composition Patterns

### Gate and tag

Require a field, then tag events that pass the check:

```toml
plugins = [
  { driver = "require_payload_field", pointer = "/organization/id" },
  { driver = "add_meta_flag", flag = "org-event" },
]
```

### Normalise event types

Remap multiple source event names to a canonical vocabulary:

```toml
plugins = [
  { driver = "event_type_alias", from = "push", to = "git.push" },
  { driver = "event_type_alias", from = "create", to = "git.ref.create" },
]
```

Plugins run in order, so only the first matching alias fires per event.

### Structured filtering at egress

On a smash adapter, require that events include a specific field before delivery:

```toml
[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"
url = "http://127.0.0.1:18789/hooks/agent"
token_env = "OPENCLAW_WEBHOOK_TOKEN"
timeout_seconds = 20
max_retries = 5
plugins = [
  { driver = "require_payload_field", pointer = "/action" },
]
```

Events without an `action` field are rejected before delivery to OpenClaw.

---

## Plugin Validation

Plugin config is validated at startup for adapters that are **active in the selected profile**. Validation errors are security-critical and block startup. Inactive adapter plugins are not validated.

Validation checks per plugin:
- `event_type_alias`: `from` and `to` must both be non-empty strings.
- `require_payload_field`: `pointer` must be non-empty and start with `/`.
- `add_meta_flag`: `flag` must be non-empty.

Unknown plugin drivers are treated as validation errors.
