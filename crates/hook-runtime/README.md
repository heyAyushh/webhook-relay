# hook-runtime

Reusable runtime execution engine for hook smash pipelines and adapters.

## Responsibilities

- Load smash runtime configuration from environment.
- Validate adapter and route configuration fail-closed.
- Consume Kafka messages and route by topic/event filters.
- Execute destination adapters with required/optional semantics.
- Publish DLQ envelopes for required-path failures.
- Apply smash adapter plugins before delivery.

## Main Paths

- `src/smash/config.rs`: runtime config + validation
- `src/smash/consumer.rs`: Kafka consume loop and route delivery
- `src/smash/dlq.rs`: DLQ producer
- `src/adapters/egress/*`: egress adapter drivers

## Smash Adapter Plugins

Supported plugin drivers:
- `event_type_alias`
- `require_payload_field`
- `add_meta_flag`

Execution is per destination adapter and in declaration order.

## Integration

Used by:
- `apps/kafka-openclaw-hook` (compatibility wrapper binary)

## Test

```bash
cargo test -p hook-runtime
```
