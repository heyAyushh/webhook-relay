# Deployment

## Deployment Options

| Mode | When to use |
|---|---|
| Local dev (direct) | Development, testing |
| systemd (bare metal) | Single server production |
| Firecracker microVMs | Isolated production deployment |

---

## Local Development

### Prerequisites

- Rust toolchain (see `rust-toolchain.toml`)
- A running Kafka broker (local or remote)
- `.env` file with required variables

### Setup

```bash
cp .env.default .env
# Edit .env with your KAFKA_BROKERS, HMAC secrets, etc.

# Install the hook CLI
cargo install --path tools/hook

# Create Kafka topics
KAFKA_BOOTSTRAP=127.0.0.1:9092 \
SOURCES="github linear" \
  skills/kafka-topic-setup/scripts/create-hook-topics.sh
```

### Run the stack

Each role runs in a separate terminal:

```bash
# Terminal 1: serve
hook serve --app default-openclaw

# Terminal 2: relay
hook relay \
  --topics webhooks.github,webhooks.linear \
  --output-topic webhooks.core

# Terminal 3: smash
hook smash --app default-openclaw
```

Or use the stack script:

```bash
scripts/run-hook-stack.sh
```

### Test a webhook

```bash
# Send a test GitHub ping
curl -X POST http://localhost:8080/webhook/github \
  -H "Content-Type: application/json" \
  -H "X-GitHub-Event: ping" \
  -H "X-GitHub-Delivery: test-delivery-1" \
  -H "X-Hub-Signature-256: sha256=<computed>" \
  -d '{"zen":"Keep it logically awesome."}'
```

---

## systemd (Bare Metal)

### Binary install

Build the release binaries:

```bash
scripts/build-release-binaries.sh
```

Archives are written to `dist/releases/`. Extract and install:

```bash
tar -xzf dist/releases/hook-<target>.tar.gz -C /usr/local/bin/
tar -xzf dist/releases/webhook-relay-<target>.tar.gz -C /usr/local/bin/
```

### Environment file

Place your env file at `/etc/relay/.env`:

```bash
mkdir -p /etc/relay
cp .env.default /etc/relay/.env
# Edit /etc/relay/.env with production values
```

### Generate TLS certificates

```bash
scripts/gen-certs.sh
# Certificates written to /etc/relay/certs/ by default
```

### systemd units

Create a unit for each role. Example for serve:

```ini
# /etc/systemd/system/hook-serve.service
[Unit]
Description=hook serve (webhook ingress)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=hook
Group=hook
EnvironmentFile=/etc/relay/.env
ExecStart=/usr/local/bin/hook serve --app default-openclaw
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

Repeat for relay and smash:

```bash
# relay
ExecStart=/usr/local/bin/hook relay \
  --topics webhooks.github,webhooks.linear \
  --output-topic webhooks.core

# smash
ExecStart=/usr/local/bin/hook smash --app default-openclaw
```

Enable and start:

```bash
systemctl daemon-reload
systemctl enable --now hook-serve hook-relay hook-smash
```

### Kafka via systemd

If running Kafka via the Firecracker skill, the Kafka VM is managed by `firecracker@kafka.service`. See `skills/kafka-kraft-firecracker/SKILL.md` for setup.

---

## Firecracker microVMs

The repo ships a full Firecracker-based deployment stack for binary-first isolation. Two VMs run on the host: one for the hook relay service and one for Kafka.

### Host prerequisites

- Linux host with `/dev/kvm`
- `firecracker` binary on PATH
- `socat` (if proxy-mux is enabled)

### Network setup

```bash
sudo scripts/setup-firecracker-bridge-network.sh
```

This creates:
- Bridge `br-relay` for the relay VM (`172.30.0.0/24`)
- TAP `tap-kafka0` for the Kafka VM (`172.16.40.0/24`)
- NAT rules for outbound connectivity

### Kafka VM

Follow the `kafka-kraft-firecracker` skill to boot and bootstrap the Kafka guest:

```bash
# Boot Kafka VM
scripts/run-firecracker.sh --config out/firecracker/kafka-config.json

# Bootstrap Kafka inside guest
KAFKA_ADVERTISED_HOST=172.16.40.2 \
KAFKA_QUORUM_VOTERS="1@127.0.0.1:9093" \
  bash /tmp/bootstrap-kafka-kraft.sh
```

### Relay VM

Boot a Firecracker VM with the relay binary baked into the rootfs:

```bash
scripts/build-firecracker-rootfs.sh
scripts/run-firecracker.sh --config out/firecracker/relay-config.json
```

### Systemd orchestration

The repo ships systemd unit templates for the host:

```bash
# Copy unit files
cp firecracker/systemd/*.service /etc/systemd/system/
cp firecracker/systemd/*.timer /etc/systemd/system/
cp firecracker/systemd/runtime.env.example /etc/firecracker/runtime.env

# Set repo root
echo "FIRECRACKER_REPO_ROOT=/opt/webhook-relay" > /etc/firecracker/runtime.env

systemctl daemon-reload
systemctl enable --now firecracker-network.service
systemctl enable --now firecracker@relay.service
systemctl enable --now firecracker@kafka.service
```

### Broker inventory

Add your Kafka VM to `/etc/firecracker/brokers.json`:

```json
{
  "brokers": [
    {
      "id": "kafka",
      "node_id": 1,
      "ip": "172.16.40.2",
      "tap": "tap-kafka0",
      "socket": "/tmp/kafka-fc.sock",
      "host_proxy_port": 9092,
      "config": "/opt/firecracker/kafka/config.json",
      "rootfs": "/opt/firecracker/kafka/rootfs.ext4"
    }
  ]
}
```

### Proxy mux (optional)

Enable host-side port forwarding so relay VM and host processes can reach Kafka via localhost:

```bash
# /etc/firecracker/proxy-mux.env
FIRECRACKER_ENABLE_BROKER_PROXIES=true
```

```bash
systemctl enable --now firecracker-proxy-mux.service
```

### Watchdog

Enable the watchdog for auto-recovery and alerting:

```bash
cp firecracker/watchdog/watchdog.env.example /etc/firecracker/watchdog.env
cp firecracker/watchdog/alerts.env.example /etc/firecracker/alerts.env
# Edit both files with your alert endpoints

systemctl enable --now firecracker-watchdog.timer
```

The watchdog runs every minute, port-probes relay and broker VMs, detects stuck processes, and optionally sends alerts via webhook or email.

### Network teardown

```bash
sudo scripts/teardown-firecracker-bridge-network.sh
```

---

## Configuration File Locations (Production Reference)

| File | Purpose |
|---|---|
| `/etc/relay/.env` | Environment variables for all hook roles |
| `/etc/relay/certs/relay.crt` | Client TLS certificate |
| `/etc/relay/certs/relay.key` | Client TLS private key |
| `/etc/relay/certs/ca.crt` | CA certificate |
| `/etc/firecracker/brokers.json` | Kafka broker inventory (Firecracker deployments) |
| `/etc/firecracker/proxy-mux.env` | Proxy mux settings |
| `/etc/firecracker/watchdog.env` | Watchdog settings |
| `/etc/firecracker/alerts.env` | Alert delivery settings |
| `apps/<app>/contract.toml` | App contract (typically in repo, not /etc) |
| `config/kafka-core.toml` | Kafka core TOML config (optional, env takes precedence) |
