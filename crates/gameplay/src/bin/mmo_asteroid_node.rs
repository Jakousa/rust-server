use mmo_core::BoundingBox3D;
use mmo_gameplay::asteroid::bootstrap_asteroid_world;
use mmo_node::{run_node_daemon_with_sim, TickSimulation};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node_id: u16 = std::env::var("NODE_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse()?;

    // Default asteroid sector world bounds (-100.0 to 100.0 across X/Y/Z)
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let mut tick_sim = TickSimulation::new(node_id, bounds);
    bootstrap_asteroid_world(&mut tick_sim);

    println!(
        "[Asteroid Node {}] Authoritative 6DoF Asteroid Sector bootstrapped (Home Station + Voxel Asteroids)",
        node_id
    );

    run_node_daemon_with_sim(node_id, bounds, tick_sim).await
}
