//! Reusable test cluster topology builder for cluster integration tests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mmo_core::guid::Guid;
use mmo_core::BoundingBox3D;
use mmo_director::NodeRegistry;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::{GatewayServer, GatewayServiceImpl, PacketDispatcher};
use mmo_node::network::{GhostStore, NodeServiceImpl};
use mmo_node::sim::TickSimulation;

/// Configuration for a simulation node in a test cluster.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub id: u16,
    pub bounds: Vec<BoundingBox3D>,
    pub addr: Option<String>,
}

/// A simulated node fixture managed by [`TestCluster`].
pub struct TestNode {
    pub id: u16,
    pub bounds: Vec<BoundingBox3D>,
    pub addr: String,
    pub socket_addr: Option<SocketAddr>,
    pub sim: Arc<tokio::sync::RwLock<TickSimulation>>,
    pub ghost_store: Arc<tokio::sync::RwLock<GhostStore>>,
}

impl TestNode {
    /// Acquires a write lock on the node's simulation instance.
    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, TickSimulation> {
        self.sim.write().await
    }

    /// Acquires a read lock on the node's simulation instance.
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, TickSimulation> {
        self.sim.read().await
    }

    /// Returns the primary bounding box for this node.
    pub fn primary_bounds(&self) -> BoundingBox3D {
        self.bounds.first().copied().unwrap_or(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ))
    }
}

/// Fluent builder for setting up edge gateways, node registries, and simulation nodes.
pub struct TestClusterBuilder {
    nodes: Vec<NodeConfig>,
    spawn_grpc: bool,
    spawn_gateway_grpc: bool,
}

impl Default for TestClusterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestClusterBuilder {
    /// Creates a new `TestClusterBuilder` with gRPC enabled by default.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            spawn_grpc: true,
            spawn_gateway_grpc: false,
        }
    }

    /// Adds a simulation node with single bounding box.
    pub fn with_node(mut self, node_id: u16, bounds: BoundingBox3D) -> Self {
        self.nodes.push(NodeConfig {
            id: node_id,
            bounds: vec![bounds],
            addr: None,
        });
        self
    }

    /// Adds a simulation node with multiple bounding boxes.
    pub fn with_node_bounds(mut self, node_id: u16, bounds: Vec<BoundingBox3D>) -> Self {
        self.nodes.push(NodeConfig {
            id: node_id,
            bounds,
            addr: None,
        });
        self
    }

    /// Adds a simulation node with custom address.
    pub fn with_node_and_addr(
        mut self,
        node_id: u16,
        bounds: BoundingBox3D,
        addr: impl Into<String>,
    ) -> Self {
        self.nodes.push(NodeConfig {
            id: node_id,
            bounds: vec![bounds],
            addr: Some(addr.into()),
        });
        self
    }

    /// Configures whether background gRPC servers are spawned for simulation nodes.
    pub fn with_spawn_grpc(mut self, spawn: bool) -> Self {
        self.spawn_grpc = spawn;
        self
    }

    /// Configures whether a background gRPC server is spawned for the Edge Gateway.
    pub fn with_gateway_grpc(mut self, spawn: bool) -> Self {
        self.spawn_gateway_grpc = spawn;
        self
    }

    /// Asynchronously builds and spawns all cluster components.
    pub async fn build(self) -> TestCluster {
        let mut registry = NodeRegistry::new();
        let router = Arc::new(SessionRouter::new());
        let gateway_server = Arc::new(GatewayServer::new(Arc::clone(&router)));

        let mut gateway_grpc_addr = None;
        if self.spawn_gateway_grpc {
            let gw_listener = std::net::TcpListener::bind("127.0.0.1:0")
                .expect("Failed to bind ephemeral port for gateway gRPC");
            let addr = gw_listener
                .local_addr()
                .expect("Failed to get local addr for gateway gRPC");
            drop(gw_listener);

            let gateway_grpc = GatewayServiceImpl::new(Arc::clone(&router));
            tokio::spawn(async move {
                let _ = tonic::transport::Server::builder()
                    .add_service(gateway_grpc.into_server())
                    .serve(addr)
                    .await;
            });
            gateway_grpc_addr = Some(addr);
        }

        let mut nodes = HashMap::new();

        for config in self.nodes {
            let primary_bound = config.bounds.first().copied().unwrap_or(BoundingBox3D::new(
                (-100.0, -100.0, -100.0),
                (100.0, 100.0, 100.0),
            ));
            let sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
                config.id,
                primary_bound,
            )));

            let (addr_str, socket_addr) = if self.spawn_grpc {
                let listener = match &config.addr {
                    Some(custom_addr) => std::net::TcpListener::bind(custom_addr)
                        .expect("Failed to bind configured node addr"),
                    None => std::net::TcpListener::bind("127.0.0.1:0")
                        .expect("Failed to bind ephemeral node addr"),
                };
                let local_addr = listener
                    .local_addr()
                    .expect("Failed to read node local_addr");
                drop(listener);

                let service = NodeServiceImpl::new(Arc::clone(&sim));
                tokio::spawn(async move {
                    let _ = tonic::transport::Server::builder()
                        .add_service(service.into_server())
                        .serve(local_addr)
                        .await;
                });
                (local_addr.to_string(), Some(local_addr))
            } else {
                let addr = config
                    .addr
                    .unwrap_or_else(|| format!("127.0.0.1:{}", 50050 + config.id));
                (addr, None)
            };

            registry.register_node_with_id(
                config.id as u32,
                addr_str.clone(),
                config.bounds.clone(),
            );

            nodes.insert(
                config.id,
                TestNode {
                    id: config.id,
                    bounds: config.bounds,
                    addr: addr_str,
                    socket_addr,
                    sim,
                    ghost_store: Arc::new(tokio::sync::RwLock::new(GhostStore::new())),
                },
            );
        }

        if self.spawn_grpc || self.spawn_gateway_grpc {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        TestCluster {
            registry,
            router,
            gateway_server,
            gateway_grpc_addr,
            nodes,
        }
    }
}

/// A running cluster test fixture containing Director registry, Edge Gateway, and Simulation Nodes.
pub struct TestCluster {
    pub registry: NodeRegistry,
    pub router: Arc<SessionRouter>,
    pub gateway_server: Arc<GatewayServer>,
    pub gateway_grpc_addr: Option<SocketAddr>,
    pub nodes: HashMap<u16, TestNode>,
}

impl TestCluster {
    /// Returns a new [`TestClusterBuilder`].
    pub fn builder() -> TestClusterBuilder {
        TestClusterBuilder::new()
    }

    /// Returns a reference to the gateway's [`SessionRouter`].
    pub fn router(&self) -> &Arc<SessionRouter> {
        &self.router
    }

    /// Returns a reference to the [`GatewayServer`].
    pub fn gateway(&self) -> &Arc<GatewayServer> {
        &self.gateway_server
    }

    /// Creates a [`PacketDispatcher`] backed by this cluster's router.
    pub fn dispatcher(&self) -> PacketDispatcher {
        self.gateway_server.dispatcher().clone()
    }

    /// Returns a reference to the specified [`TestNode`].
    pub fn node(&self, id: u16) -> &TestNode {
        self.nodes.get(&id).expect("Node not found in TestCluster")
    }

    /// Returns a mutable reference to the specified [`TestNode`].
    pub fn node_mut(&mut self, id: u16) -> &mut TestNode {
        self.nodes
            .get_mut(&id)
            .expect("Node not found in TestCluster")
    }

    /// Returns an `Arc<RwLock<TickSimulation>>` clone for the specified node.
    pub fn node_sim(&self, id: u16) -> Arc<tokio::sync::RwLock<TickSimulation>> {
        Arc::clone(&self.node(id).sim)
    }

    /// Returns an `Arc<RwLock<GhostStore>>` clone for the specified node.
    pub fn node_ghost_store(&self, id: u16) -> Arc<tokio::sync::RwLock<GhostStore>> {
        Arc::clone(&self.node(id).ghost_store)
    }

    /// Returns the network address string for the specified node.
    pub fn node_addr(&self, id: u16) -> &str {
        &self.node(id).addr
    }

    /// Returns the gateway's gRPC socket address. Panics if gateway gRPC was not enabled.
    pub fn gateway_grpc_addr(&self) -> SocketAddr {
        self.gateway_grpc_addr
            .expect("gateway_grpc_addr requested but gateway gRPC server was not spawned")
    }

    /// Registers a client session routed to the specified simulation node.
    pub fn register_session(&self, session_id: u64, guid: Guid, node_id: u16) {
        self.router
            .register_session(session_id, guid, self.node_addr(node_id).to_string());
    }
}
