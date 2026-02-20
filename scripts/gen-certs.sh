#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_CERT_DIR="certs"
readonly CA_COMMON_NAME="OpenClaw-AutoMQ-CA"
readonly CERT_VALID_DAYS="825"
readonly KEY_BITS="4096"

CERT_DIR="${1:-$DEFAULT_CERT_DIR}"

log() {
  printf '%s\n' "$*"
}

require_cmd() {
  local cmd="$1"
  command -v "$cmd" >/dev/null 2>&1 || {
    log "error: missing required command: $cmd"
    exit 1
  }
}

write_client_cert() {
  local name="$1"
  local key_file="$CERT_DIR/${name}.key"
  local csr_file="$CERT_DIR/${name}.csr"
  local crt_file="$CERT_DIR/${name}.crt"
  local ext_file="$CERT_DIR/${name}.ext"

  openssl genrsa -out "$key_file" "$KEY_BITS"
  openssl req -new -key "$key_file" -subj "/CN=${name}" -out "$csr_file"

  cat > "$ext_file" <<'EOF_EXT'
keyUsage = critical,digitalSignature,keyEncipherment
extendedKeyUsage = clientAuth
subjectAltName = DNS:localhost
EOF_EXT

  openssl x509 -req \
    -in "$csr_file" \
    -CA "$CERT_DIR/ca.crt" \
    -CAkey "$CERT_DIR/ca.key" \
    -CAcreateserial \
    -out "$crt_file" \
    -days "$CERT_VALID_DAYS" \
    -sha256 \
    -extfile "$ext_file"

  rm -f "$csr_file" "$ext_file"
}

main() {
  require_cmd "openssl"

  mkdir -p "$CERT_DIR"
  chmod 700 "$CERT_DIR"

  openssl genrsa -out "$CERT_DIR/ca.key" "$KEY_BITS"
  openssl req -x509 -new -nodes \
    -key "$CERT_DIR/ca.key" \
    -sha256 \
    -days "$CERT_VALID_DAYS" \
    -subj "/CN=${CA_COMMON_NAME}" \
    -out "$CERT_DIR/ca.crt"

  write_client_cert "relay"
  write_client_cert "consumer"

  chmod 600 "$CERT_DIR"/*.key
  chmod 644 "$CERT_DIR"/*.crt

  log "certificates generated in: $CERT_DIR"
  log "- CA:       $CERT_DIR/ca.crt"
  log "- relay:    $CERT_DIR/relay.crt + $CERT_DIR/relay.key"
  log "- consumer: $CERT_DIR/consumer.crt + $CERT_DIR/consumer.key"
}

main "$@"
