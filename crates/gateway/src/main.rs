use std::sync::{Arc, RwLock};
use mmo_gateway::{GatewayServer, SessionRouter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let router = Arc::new(RwLock::new(SessionRouter::new()));
    let server = GatewayServer::new(router);

    println!("MMO Edge Gateway initialized on 0.0.0.0:8080");
    let _listener = server.bind("0.0.0.0:8080").await?;

    Ok(())
}
