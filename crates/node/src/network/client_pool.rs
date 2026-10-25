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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_pool_caching_and_invalidation() {
        let pool = GrpcClientPool::new();
        let endpoint = "127.0.0.1:50051";

        // First lazy retrieval inserts into cache
        let ch1 = pool.get_channel(endpoint).expect("valid endpoint");
        assert_eq!(pool.channels.len(), 1);

        // Second retrieval returns cached channel
        let ch2 = pool.get_channel(endpoint).expect("returns cached");
        assert_eq!(pool.channels.len(), 1);
        drop((ch1, ch2));

        // Invalidation removes by normalized http:// URI
        pool.invalidate(endpoint);
        assert_eq!(pool.channels.len(), 0);

        // Clearing all channels
        let _ = pool.get_channel("http://127.0.0.1:50052");
        let _ = pool.get_channel("http://127.0.0.1:50053");
        assert_eq!(pool.channels.len(), 2);
        pool.clear();
        assert_eq!(pool.channels.len(), 0);
    }

    #[test]
    fn test_client_pool_invalid_endpoint_error() {
        let pool = GrpcClientPool::new();
        // Completely invalid URI (control characters or invalid scheme syntax)
        let res = pool.get_channel("http://[::1:::invalid");
        assert!(res.is_err());
    }
}
