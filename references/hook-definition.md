# Hook Definition Reference

## Table of Contents
- [All Hook Fields](#all-hook-fields)
- [Trigger Rules](#trigger-rules)
- [Match Types](#match-types)
- [Parameter Sources](#parameter-sources)
- [Template Functions](#template-functions)
- [CLI Flags](#cli-flags)

## All Hook Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | **Required.** Hook ID, creates endpoint `/hooks/{id}` |
| `execute-command` | string | **Required.** Path to command/script to execute |
| `command-working-directory` | string | Working directory for command execution |
| `response-message` | string | String returned to HTTP caller |
| `response-headers` | array | Custom response headers `[{name, value}]` |
| `success-http-response-code` | int | HTTP status on success (default: 200) |
| `trigger-rule-mismatch-http-response-code` | int | HTTP status when trigger rule fails |
| `incoming-payload-content-type` | string | Override Content-Type detection |
| `http-methods` | array | Allowed HTTP methods (default: POST) |
| `include-command-output-in-response` | bool | Return stdout in HTTP response |
| `include-command-output-in-response-on-error` | bool | Return stderr on failure (requires above) |
| `pass-arguments-to-command` | array | Positional args from request data |
| `pass-environment-to-command` | array | Env vars from request data |
| `pass-file-to-command` | array | File data with optional base64 decode |
| `parse-parameters-as-json` | array | Parse JSON string fields as objects |
| `trigger-rule` | object | Conditional execution logic |
| `trigger-signature-soft-failures` | bool | Allow signature failures in OR rules |

## Trigger Rules

Rules compose with logical operators:

```yaml
# AND - all must match
trigger-rule:
  and:
    - match: {type: value, ...}
    - match: {type: value, ...}

# OR - any must match
trigger-rule:
  or:
    - match: {type: value, ...}
    - match: {type: value, ...}

# NOT - must not match
trigger-rule:
  not:
    match: {type: value, ...}

# Nested
trigger-rule:
  and:
    - match: {type: payload-hmac-sha256, ...}
    - or:
        - match: {type: value, ...}
        - match: {type: value, ...}
```

## Match Types

### value - Exact string match
```yaml
match:
  type: value
  value: "pull_request"
  parameter:
    source: header
    name: X-GitHub-Event
```

### regex - Pattern match (Go regexp)
```yaml
match:
  type: regex
  regex: "^refs/heads/(main|develop)$"
  parameter:
    source: payload
    name: ref
```

### payload-hmac-sha256 - HMAC signature verification
```yaml
match:
  type: payload-hmac-sha256
  secret: "{{ getenv \"WEBHOOK_SECRET\" }}"
  parameter:
    source: header
    name: X-Hub-Signature-256
```
Also: `payload-hmac-sha1`, `payload-hmac-sha512`.

### ip-whitelist - CIDR range check
```yaml
match:
  type: ip-whitelist
  ip-range: "192.168.0.0/16"
```

## Parameter Sources

```yaml
# Payload field (dot-notation for nesting, 0-indexed arrays)
- source: payload
  name: pull_request.head.ref
  envname: PR_BRANCH          # for pass-environment-to-command

# HTTP header
- source: header
  name: X-GitHub-Event

# Query string (?key=value)
- source: url
  name: token

# Request metadata (only "method" or "remote-addr")
- source: request
  name: remote-addr

# Entire payload/headers/query as JSON string
- source: entire-payload
- source: entire-headers
- source: entire-query
```

## Template Functions

Hook config files support Go templates:

| Function | Example |
|----------|---------|
| `getenv` | `"{{ getenv \"SECRET\" }}"` |

Use `getenv` to reference secrets without hardcoding them in config.

## CLI Flags

```bash
webhook \
  -hooks hooks.yaml \       # hook config file (JSON or YAML)
  -port 9000 \              # listen port (default 9000)
  -ip "0.0.0.0" \           # bind address
  -verbose \                # verbose logging
  -hotreload \              # reload hooks on file change
  -secure \                 # use HTTPS
  -cert /path/to/cert.pem \ # TLS cert
  -key /path/to/key.pem \   # TLS key
  -urlprefix webhooks \      # URL prefix (default: hooks)
  -logfile /var/log/webhook.log
```
