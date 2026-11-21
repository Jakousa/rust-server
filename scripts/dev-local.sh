#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"
BIN_DIR="${TARGET_DIR}/debug"

usage() {
    cat <<EOF
Usage: $0 [asteroid|village|both]

Start a local MMO development stack with isolated Director, Simulation Node,
Edge Gateway, and Vite Web Inspector (with GATEWAY=local proxy).

Modes:
  asteroid   Start the Asteroid 6DoF stack + Vite dev server (default)
  village    Start the Village RPG stack + Vite dev server
  both       Start both Village and Asteroid stacks on non-overlapping ports + Vite

Options:
  -h, --help Show this help message and exit
EOF
}

MODE="asteroid"
if [ "$#" -gt 1 ]; then
    echo "Error: expected at most 1 argument, got $#" >&2
    usage >&2
    exit 1
elif [ "$#" -eq 1 ]; then
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        asteroid|village|both)
            MODE="$1"
            ;;
        *)
            echo "Error: invalid mode '$1'" >&2
            usage >&2
            exit 1
            ;;
    esac
fi

PIDS=()
CLEANED_UP=0

cleanup() {
    if [ "${CLEANED_UP}" -eq 1 ]; then
        return
    fi
    CLEANED_UP=1

    local active_pids=()
    local pid
    for pid in "${PIDS[@]:-}"; do
        if [ -n "${pid}" ] && kill -0 "${pid}" 2>/dev/null; then
            active_pids+=("${pid}")
        fi
    done

    # Also capture any remaining child jobs spawned in this shell
    while IFS= read -r pid; do
        if [ -n "${pid}" ] && kill -0 "${pid}" 2>/dev/null; then
            active_pids+=("${pid}")
        fi
    done < <(jobs -p 2>/dev/null || true)

    if [ "${#active_pids[@]}" -gt 0 ]; then
        echo ""
        echo "[dev-local] Shutting down child processes..."
        kill -TERM "${active_pids[@]}" 2>/dev/null || true

        # Give processes a moment to drain gracefully before forcing termination
        local waited=0
        while [ "${waited}" -lt 20 ]; do
            local any_alive=0
            for pid in "${active_pids[@]}"; do
                if kill -0 "${pid}" 2>/dev/null; then
                    any_alive=1
                    break
                fi
            done
            if [ "${any_alive}" -eq 0 ]; then
                break
            fi
            sleep 0.1
            waited=$((waited + 1))
        done

        for pid in "${active_pids[@]}"; do
            if kill -0 "${pid}" 2>/dev/null; then
                kill -KILL "${pid}" 2>/dev/null || true
            fi
        done
    fi

    wait 2>/dev/null || true
}

on_signal() {
    cleanup
    exit 130
}

trap cleanup EXIT
trap on_signal INT TERM

cd "${REPO_ROOT}"

echo "[dev-local] Building server binaries (mmo-director, mmo-gateway, mmo-gameplay)..."
cargo build -p mmo-director -p mmo-gateway -p mmo-gameplay --bins

start_village_stack() {
    echo "[dev-local] Starting Village stack..."
    LISTEN_ADDR="127.0.0.1:50051" \
    ADMIN_ADDR="127.0.0.1:9191" \
    GATEWAY_GRPC_ADDR="http://127.0.0.1:50050" \
        "${BIN_DIR}/mmo-director" &
    PIDS+=("$!")

    NODE_ID="1" \
    LISTEN_ADDR="127.0.0.1:50052" \
    ADMIN_ADDR="127.0.0.1:9192" \
    DIRECTOR_ADDR="http://127.0.0.1:50051" \
    GATEWAY_GRPC_ADDR="http://127.0.0.1:50050" \
        "${BIN_DIR}/mmo-village-node" &
    PIDS+=("$!")

    GATEWAY_ID="gw-village-local" \
    LISTEN_ADDR="127.0.0.1:8080" \
    PROBE_ADDR="127.0.0.1:8081" \
    GRPC_ADDR="127.0.0.1:50050" \
    WEBTRANSPORT_ADDR="0.0.0.0:4433" \
    WEBSOCKET_ADDR="0.0.0.0:8443" \
    ADMIN_ADDR="0.0.0.0:9091" \
    DIRECTOR_ADDR="http://127.0.0.1:50051" \
    DEFAULT_NODE_ADDR="127.0.0.1:50052" \
        "${BIN_DIR}/mmo-gateway" &
    PIDS+=("$!")
}

start_asteroid_stack() {
    echo "[dev-local] Starting Asteroid stack..."
    LISTEN_ADDR="127.0.0.1:51051" \
    ADMIN_ADDR="127.0.0.1:9291" \
    GATEWAY_GRPC_ADDR="http://127.0.0.1:51050" \
        "${BIN_DIR}/mmo-director" &
    PIDS+=("$!")

    NODE_ID="1" \
    LISTEN_ADDR="127.0.0.1:51052" \
    ADMIN_ADDR="127.0.0.1:9292" \
    DIRECTOR_ADDR="http://127.0.0.1:51051" \
    GATEWAY_GRPC_ADDR="http://127.0.0.1:51050" \
        "${BIN_DIR}/mmo-asteroid-node" &
    PIDS+=("$!")

    GATEWAY_ID="gw-asteroid-local" \
    LISTEN_ADDR="127.0.0.1:8090" \
    PROBE_ADDR="127.0.0.1:8091" \
    GRPC_ADDR="127.0.0.1:51050" \
    WEBTRANSPORT_ADDR="0.0.0.0:4434" \
    WEBSOCKET_ADDR="0.0.0.0:8444" \
    ADMIN_ADDR="0.0.0.0:9092" \
    DIRECTOR_ADDR="http://127.0.0.1:51051" \
    DEFAULT_NODE_ADDR="127.0.0.1:51052" \
        "${BIN_DIR}/mmo-gateway" &
    PIDS+=("$!")
}

case "${MODE}" in
    village)
        start_village_stack
        ;;
    asteroid)
        start_asteroid_stack
        ;;
    both)
        start_village_stack
        start_asteroid_stack
        ;;
esac

echo ""
echo "============================================================"
echo "  MMO Local Dev Stack Ready (mode: ${MODE})"
echo "============================================================"
if [ "${MODE}" = "asteroid" ] || [ "${MODE}" = "both" ]; then
    echo "  Asteroid Inspector : http://localhost:3000/asteroid"
fi
if [ "${MODE}" = "village" ] || [ "${MODE}" = "both" ]; then
    echo "  Village Inspector  : http://localhost:3000/village"
fi
echo "  Press Ctrl-C to stop all services."
echo "============================================================"
echo ""

GATEWAY=local npm --prefix web run dev -- --port 3000 &
PIDS+=("$!")

wait
