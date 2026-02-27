#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_CHROOT_BASE="/srv/jailer"
readonly DEFAULT_FIRECRACKER_EXEC="/usr/local/bin/firecracker"
readonly DEFAULT_BOOT_ARGS_FILE="boot_args.default"
readonly DEFAULT_JAILER_DEFAULTS_FILE="jailer.defaults.json"
readonly DEFAULT_RATELIMITS_DEFAULTS_FILE="ratelimits.default.json"
readonly DEFAULT_JAILER_PARENT_CGROUP="firecracker"
readonly DEFAULT_JAILER_UID="0"
readonly DEFAULT_JAILER_GID="0"
readonly DEFAULT_VM_SOCKET_DIR="/tmp"
readonly COPY_DISABLED=0
readonly COPY_ENABLED=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

CHROOT_BASE="${FIRECRACKER_CHROOT_BASE:-${DEFAULT_CHROOT_BASE}}"
FIRECRACKER_EXEC="${FIRECRACKER_EXEC:-${DEFAULT_FIRECRACKER_EXEC}}"
JAILER_PARENT_CGROUP="${FIRECRACKER_JAILER_PARENT_CGROUP:-${DEFAULT_JAILER_PARENT_CGROUP}}"
JAILER_UID="${FIRECRACKER_JAILER_UID:-${DEFAULT_JAILER_UID}}"
JAILER_GID="${FIRECRACKER_JAILER_GID:-${DEFAULT_JAILER_GID}}"
BOOT_ARGS_FILE="${FIRECRACKER_BOOT_ARGS_FILE:-${SCRIPT_DIR}/${DEFAULT_BOOT_ARGS_FILE}}"
JAILER_DEFAULTS_FILE="${FIRECRACKER_JAILER_DEFAULTS_FILE:-${SCRIPT_DIR}/${DEFAULT_JAILER_DEFAULTS_FILE}}"
RATELIMITS_DEFAULTS_FILE="${FIRECRACKER_RATELIMITS_FILE:-${SCRIPT_DIR}/${DEFAULT_RATELIMITS_DEFAULTS_FILE}}"
COPY_KERNEL=${FC_KERNEL_COPY:-${COPY_DISABLED}}
COPY_DRIVES=${FC_DRIVE_COPY:-${COPY_DISABLED}}
COPY_CONFIG=${FC_COPY_CONFIG:-${COPY_ENABLED}}
NETWORK_NAMESPACE="${FC_NETNS:-}"

VM_ID=""
CONFIG_PATH=""
SOCKET_PATH=""
JAILER_BIN=""
JAIL_ROOT=""
JAIL_SOCKET=""
HOST_SOCKET_SYMLINK=""

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

usage() {
  cat <<'EOF_USAGE' >&2
Usage: firecracker/runtime/launch.sh <vm_id> <config_path> [socket_path]

Environment overrides:
  FIRECRACKER_CHROOT_BASE        Jailer chroot base directory
  FIRECRACKER_EXEC               Firecracker binary path used by jailer
  FIRECRACKER_BOOT_ARGS_FILE     Default boot args file
  FIRECRACKER_JAILER_DEFAULTS_FILE
  FIRECRACKER_RATELIMITS_FILE
  FIRECRACKER_JAILER_PARENT_CGROUP
  FIRECRACKER_JAILER_UID         Jailer UID
  FIRECRACKER_JAILER_GID         Jailer GID
  FC_KERNEL_COPY=1               Copy kernel into jail instead of bind-mount
  FC_DRIVE_COPY=1                Copy drives into jail instead of bind-mount
  FC_COPY_CONFIG=0               Skip copying source config snapshot
  FC_NETNS=<path>                Optional network namespace for jailer
EOF_USAGE
}

require_cmd() {
  local command_name="$1"
  command -v "${command_name}" >/dev/null 2>&1 || die "missing command: ${command_name}"
}

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    die "launch.sh requires root (mounts + jailer)"
  fi
}

trim_whitespace() {
  local value="$1"
  printf '%s' "${value}" | xargs
}

resolve_socket_path() {
  local vm_id="$1"
  local provided_socket_path="$2"
  local default_socket_path="${DEFAULT_VM_SOCKET_DIR}/${vm_id}-fc.sock"

  if [ -n "${provided_socket_path}" ]; then
    printf '%s' "${provided_socket_path}"
    return
  fi

  case "${vm_id}" in
    kafka)
      printf '%s' "${DEFAULT_VM_SOCKET_DIR}/kafka-fc.sock"
      ;;
    relay|webhook-relay)
      printf '%s' "${DEFAULT_VM_SOCKET_DIR}/relay-fc.sock"
      ;;
    *)
      printf '%s' "${default_socket_path}"
      ;;
  esac
}

boot_args_contains_token() {
  local boot_args="$1"
  local token="$2"

  case " ${boot_args} " in
    *" ${token} "*) return 0 ;;
    *) return 1 ;;
  esac
}

append_boot_arg_if_missing() {
  local boot_args="$1"
  local candidate_token="$2"
  local candidate_key="${candidate_token%%=*}"

  if [ "${candidate_key}" != "${candidate_token}" ]; then
    if printf '%s\n' "${boot_args}" | grep -Eq "(^|[[:space:]])${candidate_key}=[^[:space:]]+"; then
      printf '%s' "${boot_args}"
      return
    fi
  else
    if boot_args_contains_token "${boot_args}" "${candidate_token}"; then
      printf '%s' "${boot_args}"
      return
    fi
  fi

  printf '%s %s' "${boot_args}" "${candidate_token}"
}

resolve_boot_args() {
  local config_path="$1"
  local default_boot_args=""
  local extra_boot_args=""
  local merged_boot_args=""
  local token=""

  default_boot_args="$(trim_whitespace "$(cat "${BOOT_ARGS_FILE}")")"
  extra_boot_args="$(jq -r '."boot-source"."boot_args" // ""' "${config_path}")"
  extra_boot_args="$(trim_whitespace "${extra_boot_args}")"

  if [ -z "${extra_boot_args}" ]; then
    die "boot_args must include init=... in config: ${config_path}"
  fi

  if ! printf '%s\n' "${extra_boot_args}" | grep -qE '(^|[[:space:]])init='; then
    die "boot_args must include init=... in config: ${config_path}"
  fi

  merged_boot_args="${extra_boot_args}"
  for token in ${default_boot_args}; do
    merged_boot_args="$(append_boot_arg_if_missing "${merged_boot_args}" "${token}")"
  done

  trim_whitespace "${merged_boot_args}"
}

cleanup_existing_jail() {
  local jail_root="$1"
  local cgroup_parent_path="/sys/fs/cgroup/${JAILER_PARENT_CGROUP}/${VM_ID}"
  local mount_point=""

  if [ -d "${jail_root}" ]; then
    mapfile -t existing_mounts < <(findmnt -R -n -o TARGET "${jail_root}" 2>/dev/null | sort -r)
    for mount_point in "${existing_mounts[@]}"; do
      umount -l "${mount_point}" || true
    done
    rm -rf "${jail_root}" || true
  fi

  if [ -e "${cgroup_parent_path}/cgroup.kill" ]; then
    printf '1' > "${cgroup_parent_path}/cgroup.kill" || true
  fi
}

ensure_jailer_binary() {
  JAILER_BIN="${JAILER_BIN:-${FIRECRACKER_JAILER_BIN:-}}"

  if [ -z "${JAILER_BIN}" ]; then
    JAILER_BIN="$(command -v jailer || true)"
  fi

  [ -x "${JAILER_BIN}" ] || die "jailer binary not found"
}

bind_or_copy_file() {
  local source_path="$1"
  local target_path="$2"
  local copy_mode="$3"

  [ -f "${source_path}" ] || die "source file not found: ${source_path}"

  mkdir -p "$(dirname "${target_path}")"
  [ -e "${target_path}" ] || touch "${target_path}"

  if [ "${copy_mode}" -eq "${COPY_ENABLED}" ]; then
    cp -f "${source_path}" "${target_path}"
  else
    mount --bind "${source_path}" "${target_path}"
  fi
}

build_drive_path_map() {
  local config_path="$1"
  local jail_opt_dir="$2"
  local drive_source_path=""
  local in_jail_drive_path=""
  local jail_drive_path=""
  local drive_counter=0
  local drive_path_map_json='{}'

  mapfile -t drive_paths < <(jq -r '.drives[]."path_on_host"' "${config_path}")
  [ "${#drive_paths[@]}" -gt 0 ] || die "config has no drives: ${config_path}"

  for drive_source_path in "${drive_paths[@]}"; do
    in_jail_drive_path="/opt/firecracker/drives/drive-${drive_counter}-$(basename "${drive_source_path}")"
    jail_drive_path="${JAIL_ROOT}${in_jail_drive_path}"
    bind_or_copy_file "${drive_source_path}" "${jail_drive_path}" "${COPY_DRIVES}"
    drive_path_map_json="$(jq --arg old "${drive_source_path}" --arg new "${in_jail_drive_path}" '. + {($old): $new}' <<<"${drive_path_map_json}")"
    drive_counter=$((drive_counter + 1))
  done

  printf '%s' "${drive_path_map_json}"
}

prepare_logging_paths_in_jail() {
  local jail_root="$1"
  local config_path="$2"
  local jail_file_path=""
  local log_path=""
  local metrics_path=""

  log_path="$(jq -r '.logger.log_path // ""' "${config_path}")"
  metrics_path="$(jq -r '.metrics.metrics_path // ""' "${config_path}")"

  for jail_file_path in "${log_path}" "${metrics_path}"; do
    if [ -z "${jail_file_path}" ] || [ "${jail_file_path}" = "null" ]; then
      continue
    fi
    mkdir -p "${jail_root}$(dirname "${jail_file_path}")"
    touch "${jail_root}${jail_file_path}"
  done
}

main() {
  require_cmd jq
  require_cmd findmnt
  require_cmd mount
  require_root

  if [ "$#" -lt 2 ]; then
    usage
    exit 1
  fi

  VM_ID="$1"
  CONFIG_PATH="$2"
  SOCKET_PATH="$(resolve_socket_path "${VM_ID}" "${3:-}")"

  [ -f "${CONFIG_PATH}" ] || die "config not found: ${CONFIG_PATH}"
  [ -f "${BOOT_ARGS_FILE}" ] || die "boot args defaults not found: ${BOOT_ARGS_FILE}"
  [ -f "${JAILER_DEFAULTS_FILE}" ] || die "jailer defaults not found: ${JAILER_DEFAULTS_FILE}"
  [ -f "${RATELIMITS_DEFAULTS_FILE}" ] || die "rate limits defaults not found: ${RATELIMITS_DEFAULTS_FILE}"
  [ -x "${FIRECRACKER_EXEC}" ] || die "firecracker exec not found or not executable: ${FIRECRACKER_EXEC}"

  ensure_jailer_binary

  local boot_args=""
  local kernel_host_path=""
  local kernel_basename=""
  local in_jail_kernel_path=""
  local jail_kernel_path=""
  local jail_exec_basename=""
  local jail_opt_dir=""
  local jail_config_path=""
  local cgroup_version=""
  local drive_path_map_json=""

  boot_args="$(resolve_boot_args "${CONFIG_PATH}")"
  kernel_host_path="$(jq -r '."boot-source"."kernel_image_path"' "${CONFIG_PATH}")"
  [ -f "${kernel_host_path}" ] || die "kernel image not found: ${kernel_host_path}"

  jail_exec_basename="$(basename "${FIRECRACKER_EXEC}")"
  JAIL_ROOT="${CHROOT_BASE}/${jail_exec_basename}/${VM_ID}/root"
  jail_opt_dir="${JAIL_ROOT}/opt/firecracker"
  jail_config_path="${jail_opt_dir}/config.json"
  JAIL_SOCKET="${JAIL_ROOT}/tmp/firecracker.socket"
  kernel_basename="$(basename "${kernel_host_path}")"
  in_jail_kernel_path="/opt/firecracker/${kernel_basename}"
  jail_kernel_path="${JAIL_ROOT}${in_jail_kernel_path}"

  cleanup_existing_jail "${JAIL_ROOT}"
  mkdir -p "${jail_opt_dir}" "${JAIL_ROOT}/dev/net" "${JAIL_ROOT}/tmp"

  bind_or_copy_file "${kernel_host_path}" "${jail_kernel_path}" "${COPY_KERNEL}"
  drive_path_map_json="$(build_drive_path_map "${CONFIG_PATH}" "${jail_opt_dir}")"

  jq \
    --arg boot_args "${boot_args}" \
    --arg kernel "${in_jail_kernel_path}" \
    --argjson drive_map "${drive_path_map_json}" \
    --slurpfile rate_limits "${RATELIMITS_DEFAULTS_FILE}" \
    '
      ."boot-source"."boot_args" = $boot_args
      | ."boot-source"."kernel_image_path" = $kernel
      | .drives |= map(
          ."path_on_host" = ($drive_map[."path_on_host"] // ."path_on_host")
          | if ($rate_limits[0].block != null) then
              ."rate_limiter" = $rate_limits[0].block
            else
              .
            end
        )
      | if ."network-interfaces" then
          ."network-interfaces" |= map(
            if ($rate_limits[0].network != null) then
              ."rx_rate_limiter" = $rate_limits[0].network.rx
              | ."tx_rate_limiter" = $rate_limits[0].network.tx
            else
              .
            end
          )
        else
          .
        end
    ' "${CONFIG_PATH}" > "${jail_config_path}"

  if [ "${COPY_CONFIG}" -eq "${COPY_ENABLED}" ]; then
    cp -f "${CONFIG_PATH}" "${jail_opt_dir}/config.src.json"
  fi

  prepare_logging_paths_in_jail "${JAIL_ROOT}" "${jail_config_path}"

  mkdir -p "$(dirname "${SOCKET_PATH}")"
  rm -f "${JAIL_SOCKET}" "${SOCKET_PATH}"
  ln -s "${JAIL_SOCKET}" "${SOCKET_PATH}"
  HOST_SOCKET_SYMLINK="${SOCKET_PATH}"

  cgroup_version="$(jq -r '.cgroup_version // "2"' "${JAILER_DEFAULTS_FILE}")"
  mapfile -t resource_limits < <(jq -r '.resource_limits | to_entries[] | "\(.key)=\(.value)"' "${JAILER_DEFAULTS_FILE}")
  mapfile -t cgroup_limits < <(jq -r '.cgroups | to_entries[] | "\(.key)=\(.value)"' "${JAILER_DEFAULTS_FILE}")

  mkdir -p "/sys/fs/cgroup/${JAILER_PARENT_CGROUP}"

  JAILER_ARGS=(
    --id "${VM_ID}"
    --exec-file "${FIRECRACKER_EXEC}"
    --uid "${JAILER_UID}"
    --gid "${JAILER_GID}"
    --chroot-base-dir "${CHROOT_BASE}"
    --cgroup-version "${cgroup_version}"
    --parent-cgroup "${JAILER_PARENT_CGROUP}"
  )

  for limit in "${resource_limits[@]}"; do
    JAILER_ARGS+=(--resource-limit "${limit}")
  done
  for cgroup_limit in "${cgroup_limits[@]}"; do
    JAILER_ARGS+=(--cgroup "${cgroup_limit}")
  done
  if [ -n "${NETWORK_NAMESPACE}" ]; then
    JAILER_ARGS+=(--netns "${NETWORK_NAMESPACE}")
  fi

  exec "${JAILER_BIN}" "${JAILER_ARGS[@]}" -- \
    --api-sock /tmp/firecracker.socket \
    --config-file /opt/firecracker/config.json
}

main "$@"
