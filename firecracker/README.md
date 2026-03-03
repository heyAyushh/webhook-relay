# Firecracker Runtime Notes

This directory contains the active Firecracker runtime stack for `hook-serve`.

## Layout

- `firecracker-config.template.json`: baseline Firecracker VM config template
- `hook-serve.service`: single-VM systemd unit template
- `runtime/*`: jailer launcher, cleanup, inventory, and defaults
- `systemd/*`: host unit/timer templates and env examples
- `watchdog/*`: local watchdog, heartbeat, boot/shutdown diagnostics, alert helpers, and external checker scripts
- `../scripts/build-firecracker-rootfs.sh`: build guest rootfs/data images
- `../scripts/run-firecracker.sh`: launch helper with config/log safety fallback
- `../scripts/setup-firecracker-bridge-network.sh`: bridge/TAP/NAT setup
- `../scripts/teardown-firecracker-bridge-network.sh`: bridge teardown

## Composable Paths

Systemd templates are configurable through `/etc/firecracker/runtime.env`:

```bash
FIRECRACKER_REPO_ROOT=/opt/hook-serve
```

Units default to `/opt/hook-serve` but can run from any location by changing `FIRECRACKER_REPO_ROOT`.

## VM Launch Flow

1. Build relay binary:

```bash
cargo build --release
```

2. Build guest images:

```bash
scripts/build-firecracker-rootfs.sh \
  --binary target/release/hook-serve \
  --rootfs out/firecracker/rootfs.ext4 \
  --data out/firecracker/data.ext4
```

3. Copy `firecracker-config.template.json` to a concrete config and update kernel/rootfs/data/network values.
4. Launch:

```bash
scripts/run-firecracker.sh --config out/firecracker/firecracker-config.json
```

`scripts/run-firecracker.sh` behavior:

- Uses `firecracker/runtime/launch.sh` by default (override with `--launcher` / `FIRECRACKER_LAUNCHER_PATH`)
- Supports direct Firecracker (`--no-launcher`)
- Rewrites runtime config log path if configured directory is unwritable
- Uses `/tmp/firecracker` fallback logs by default (override with `--fallback-log-dir`)

## Host Orchestration

Required units:

- `firecracker-network.service`
- `firecracker@.service`

Optional units:

- `firecracker-proxy-mux.service` (socat relay/broker host forwards)
- `firecracker-overwatcher.service`

Recommended setup:

1. Copy `firecracker/systemd/runtime.env.example` to `/etc/firecracker/runtime.env`.
2. Copy needed `firecracker/systemd/*.env.example` files into `/etc/firecracker/`.
3. Install unit files from `firecracker/systemd/` into `/etc/systemd/system/`.
4. Enable required units/timers.

Proxy defaults are opt-in:

- `FIRECRACKER_ENABLE_RELAY_PROXY=false`
- `FIRECRACKER_ENABLE_BROKER_PROXIES=false`

Set either to `true` in `/etc/firecracker/proxy-mux.env` if you want host-side proxying.

## Watchdog and Diagnostics

Local watchdog stack:

- `firecracker-watchdog.timer` -> `firecracker-watchdog.service`
- `firecracker-boot-logger.service`
- `firecracker-shutdown-logger.service`
- `kernel-kmsg-capture.service`
- `pstore-collect.service`

Watchdog defaults do not assume proxy/chisel:

- URL health probing is disabled unless `FIRECRACKER_WATCHDOG_RELAY_HEALTH_URL` is set
- required services default to `firecracker-network.service,firecracker@relay.service`
- chisel checks run only if `FIRECRACKER_WATCHDOG_CHISEL_HOST_PORT` is configured

Env files to copy under `/etc/firecracker/`:

- `watchdog.env`
- `alerts.env`
- `kernel-kmsg.env`
- `pstore-collect.env`

Quick status:

```bash
/opt/hook-serve/firecracker/watchdog/status.sh
```

## External Checkers (Separate Host)

- `external-blackbox.service` + `external-blackbox.timer`
- `external-chisel-check.service` + `external-chisel-check.timer`

Checker env files:

- `external-blackbox.env`
- `chisel-check.env`
- `alerts.env` (optional)

## Host Hardening Templates

- `firecracker/systemd/journald.conf.d/10-firecracker.conf`
- `firecracker/systemd/sysctl.d/99-firecracker.conf`

These are templates; review and apply per host policy.

## Security Defaults

- Keep guest rootfs read-only.
- Mount only data disk writable.
- Keep boot args with `console=ttyS0`, `panic=1`, and `init=/init`.
- Avoid embedding secrets in scripts; use `/etc/firecracker/*.env` overrides.
