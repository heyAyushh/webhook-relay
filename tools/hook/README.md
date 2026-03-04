# hook

Utility CLI for running role runtimes and operational workflows.

## Roles

- `hook serve`: runs ingress (`webhook-relay`) with contract-projected serve adapters and routes.
- `hook relay`: runs Kafka-to-Kafka core bridge.
- `hook smash`: runs smash runtime (`hook-runtime`) with contract-projected egress adapters and routes.

## Additional Commands

- `hook test`
- `hook replay`
- `hook debug`
- `hook introduce`
- `hook config`
- `hook infra`
- `hook logs`

## Global Options

- `--profile <name>` (default `default-openclaw`)
- `--app <id>`
- `--contract <path>`
- `--env-file <path>` (repeatable)
- `--config <path>`
- `--validation-mode strict|debug`
- `--force`
- `--json`

## Contract Resolution

For `serve` and `smash`:
1. `--contract <path>`
2. `--app <id>` -> `apps/<id>/contract.toml`
3. `./contract.toml`
4. embedded default-openclaw fallback

`relay` remains runtime-config driven.

## Environment Precedence

1. command flags
2. profile TOML values
3. imported `.env` files
4. process environment

## Plugin Projection

`hook serve` and `hook smash` project adapter plugin config from active contract adapters into runtime env JSON payloads.

Supported plugin drivers:
- `event_type_alias`
- `require_payload_field`
- `add_meta_flag`

## Quick Commands

```bash
cargo run -p hook -- debug capabilities
cargo run -p hook -- serve --app default-openclaw
cargo run -p hook -- relay --topics webhooks.github,webhooks.linear --output-topic webhooks.core
cargo run -p hook -- smash --app default-openclaw
cargo run -p hook -- test env
cargo run -p hook -- logs collect --scope runtime --format stream
```
