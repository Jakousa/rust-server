use dashmap::DashMap;
use std::sync::Arc;
use tonic::transport::{Channel, Endpoint};

/// Thread-safe client connection pool caching `tonic::transport::Channel` connections to peer nodes and gateways.
#[derive(Clone, Default, Debug)]
pub struct GrpcClientPool {
    channels: Arc<DashMap<String, Channel>>,
}

impl GrpcClientPool {
    /// Creates a new empty `GrpcClientPool`.
    pub fn new() -> Self {
        Self {
            channels: Arc::new(DashMap::new()),
        }
    }

    /// Retrieves an existing channel or lazily constructs a new pooled channel to `endpoint`.
    pub fn get_channel(&self, endpoint: &str) -> Result<Channel, tonic::transport::Error> {
        let endpoint_str = if endpoint.starts_with("http") {
            endpoint.to_string()
        } else {
            format!("http://{}", endpoint)
        };

        if let Some(ch) = self.channels.get(&endpoint_str) {
            return Ok(ch.value().clone());
        }

        let channel = Endpoint::from_shared(endpoint_str.clone())?.connect_lazy();
        self.channels.insert(endpoint_str, channel.clone());
        Ok(channel)
    }

    /// Retrieves an existing channel or eagerly establishes a connected channel to `endpoint`.
    pub async fn get_connected_channel(
        &self,
        endpoint: &str,
    ) -> Result<Channel, tonic::transport::Error> {
        let endpoint_str = if endpoint.starts_with("http") {
            endpoint.to_string()
        } else {
            format!("http://{}", endpoint)
        };

        if let Some(ch) = self.channels.get(&endpoint_str) {
            return Ok(ch.value().clone());
        }

        let channel = Endpoint::from_shared(endpoint_str.clone())?
            .connect()
            .await?;
        self.channels.insert(endpoint_str, channel.clone());
        Ok(channel)
    }

    /// Invalidates and removes a cached channel for an endpoint.
    pub fn invalidate(&self, endpoint: &str) {
        let endpoint_str = if endpoint.starts_with("http") {
            endpoint.to_string()
        } else {
            format!("http://{}", endpoint)
        };
        self.channels.remove(&endpoint_str);
    }

    /// Clears all cached channels in the pool.
    pub fn clear(&self) {
        self.channels.clear();
    }
}
