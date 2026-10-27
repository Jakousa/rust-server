use mmo_core::BoundingBox3D;
use mmo_gameplay::village::bootstrap_village_world;
use mmo_node::{run_node_daemon_with_sim, TickSimulation};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node_id: u16 = std::env::var("NODE_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse()?;

    // Default village world bounds (-100.0 to 100.0)
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let mut tick_sim = TickSimulation::new(node_id, bounds);
    bootstrap_village_world(&mut tick_sim);

    println!(
        "[Village Node {}] Authoritative Village world bootstrapped with NPCs and structures",
        node_id
    );

    run_node_daemon_with_sim(node_id, bounds, tick_sim).await
}
