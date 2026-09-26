#!/usr/bin/env bash
set -euo pipefail

CLUSTER_NAME="mmo-cluster"
IMAGE_TAG="mmo-server:local"

usage() {
    echo "Usage: $0 {up|down|build|import|deploy|reload|all|status|logs <app>|loadtest|chaos}"
    echo ""
    echo "Commands:"
    echo "  up       Create local k3d cluster with MMO port forwards"
    echo "  down     Tear down local k3d cluster"
    echo "  build    Build local Docker image (${IMAGE_TAG})"
    echo "  import   Import local Docker image into k3d"
    echo "  deploy   Apply Kubernetes manifests (k8s/overlays/local)"
    echo "  reload   Full fast-iteration cycle: build -> import -> deploy -> rolling restart"
    echo "  all      Full workflow: up -> build -> import -> deploy"
    echo "  status   Show status of pods, services, and statefulsets in mmo namespace"
    echo "  logs     Tail logs for a service (e.g. mmo-director, mmo-node, mmo-gateway)"
    echo "  loadtest [bots] [duration] [behavior] Run bot swarm load test (defaults: 1000 bots, 10s, hotspot)"
    echo "  chaos    Run end-to-end chaos failover & recovery test"
    exit 1
}

cmd_up() {
    if k3d cluster list | grep -q "^${CLUSTER_NAME}\b"; then
        echo "[k3d] Cluster '${CLUSTER_NAME}' already exists."
    else
        echo "[k3d] Creating cluster '${CLUSTER_NAME}'..."
        k3d cluster create "${CLUSTER_NAME}" \
            --port "8080:8080/udp@loadbalancer" \
            --port "8080:8080/tcp@loadbalancer" \
            --port "50051:50051@loadbalancer" \
            --port "9090:9090@loadbalancer" \
            --port "3000:3000@loadbalancer" \
            --k3s-arg "--disable=traefik@server:0"
        echo "[k3d] Cluster created successfully."
    fi
}

cmd_down() {
    echo "[k3d] Deleting cluster '${CLUSTER_NAME}'..."
    k3d cluster delete "${CLUSTER_NAME}" || true
}

cmd_build() {
    echo "[Docker] Building image '${IMAGE_TAG}'..."
    docker build -t "${IMAGE_TAG}" .
}

cmd_import() {
    echo "[k3d] Importing '${IMAGE_TAG}' into cluster '${CLUSTER_NAME}'..."
    k3d image import "${IMAGE_TAG}" -c "${CLUSTER_NAME}"
}

cmd_deploy() {
    echo "[k8s] Applying manifests from k8s/overlays/local..."
    kubectl apply -k k8s/overlays/local
    echo "[k8s] Deployment applied. Pods will start momentarily."
}

cmd_status() {
    kubectl get pods,svc,statefulset -n mmo
}

cmd_logs() {
    local target="${1:-}"
    if [ -z "$target" ]; then
        echo "Error: please specify app label (e.g. $0 logs mmo-node)"
        exit 1
    fi
    kubectl logs -n mmo -l "app=${target}" -f
}

cmd_reload() {
    cmd_build
    cmd_import
    cmd_deploy
    echo "[k8s] Triggering rolling restart to pick up new image..."
    kubectl rollout restart deployment/mmo-director deployment/mmo-gateway statefulset/mmo-node -n mmo
    echo "[k8s] Waiting for rollouts to settle..."
    kubectl rollout status deployment/mmo-director -n mmo --timeout=60s
    kubectl rollout status deployment/mmo-gateway -n mmo --timeout=60s
    kubectl rollout status statefulset/mmo-node -n mmo --timeout=60s
    echo "[k8s] Cluster reload complete and verified healthy."
}

cmd_loadtest() {
    local bots="${1:-1000}"
    local duration="${2:-10}"
    local behavior="${3:-hotspot}"
    echo "[LoadTest] Executing bot swarm against Gateway (127.0.0.1:8080)..."
    echo "[LoadTest] Configuration: bots=${bots}, duration=${duration}s, behavior=${behavior}"
    cargo run --release -p mmo-loadtest -- \
        --gateway-addr 127.0.0.1:8080 \
        --bots "${bots}" \
        --duration-secs "${duration}" \
        --behavior "${behavior}"
}

cmd_chaos() {
    bash "$(dirname "$0")/chaos.sh"
}

case "${1:-}" in
    up) cmd_up ;;
    down) cmd_down ;;
    build) cmd_build ;;
    import) cmd_import ;;
    deploy) cmd_deploy ;;
    reload) cmd_reload ;;
    all)
        cmd_up
        cmd_build
        cmd_import
        cmd_deploy
        ;;
    status) cmd_status ;;
    logs) shift; cmd_logs "$@" ;;
    loadtest) shift; cmd_loadtest "$@" ;;
    chaos) cmd_chaos ;;
    *) usage ;;
esac
