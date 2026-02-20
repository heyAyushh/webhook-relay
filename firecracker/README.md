# Firecracker Runtime Notes

This directory contains minimal artifacts to run `webhook-relay` directly inside a Firecracker microVM (no Docker/container runtime inside the guest).

## Files

- `firecracker-config.template.json`: baseline Firecracker VM config template
- `webhook-relay.service`: optional systemd unit for host-managed Firecracker launch
- `../scripts/build-firecracker-rootfs.sh`: build rootfs/data images containing relay binary
- `../scripts/run-firecracker.sh`: launch helper for Firecracker with generated config

## Requirements

- Linux host with KVM (`/dev/kvm` present)
- Firecracker binary and kernel image available on host
- Root/sudo for loop mount operations when creating rootfs

## Flow

1. Build relay binary:

```bash
cargo build --release
```

2. Build guest images:

```bash
scripts/build-firecracker-rootfs.sh \
  --binary target/release/webhook-relay \
  --rootfs out/firecracker/rootfs.ext4 \
  --data out/firecracker/data.ext4
```

3. Copy `firecracker-config.template.json` to a concrete config and fill values:
- kernel image path
- rootfs path
- data drive path
- network interface values
- env file path (if used by your init wrapper)

4. Launch:

```bash
scripts/run-firecracker.sh --config out/firecracker/firecracker-config.json
```

5. Route host ingress (`:443`) to guest private IP `:9000` via reverse proxy.

## Security Defaults

- Keep guest rootfs read-only.
- Mount only data disk writable (for redb).
- Run relay as PID 1 (`init=/init`) or through minimal init wrapper.
- Restrict egress to required destinations (OpenClaw private endpoint/Tailscale).
