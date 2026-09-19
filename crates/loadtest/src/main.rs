//! MMO Bot Load Test CLI Runner.

use clap::Parser;
use mmo_loadtest::swarm::{SwarmConfig, SwarmManager};
use mmo_loadtest::trajectory::WorldBounds;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "mmo-loadtest", about = "Headless Bot Swarm Load Tester for Distributed MMO Engine")]
struct Cli {
    /// Gateway network address to target
    #[arg(long, env = "GATEWAY_ADDR", default_value = "127.0.0.1:8080")]
    gateway_addr: String,

    /// Number of concurrent virtual bot clients to simulate
    #[arg(short, long, env = "BOT_COUNT", default_value = "100")]
    bots: usize,

    /// Total test duration in seconds
    #[arg(short, long, env = "DURATION_SECS", default_value = "10")]
    duration_secs: u64,

    /// Movement update rate per second (Hz)
    #[arg(long, default_value = "20")]
    tick_rate: u64,

    /// Stagger delay between connection attempts in milliseconds
    #[arg(long, default_value = "5")]
    stagger_ms: u64,

    /// Maximum movement speed (m/s)
    #[arg(long, default_value = "15.0")]
    max_speed: f32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    println!("============================================================");
    println!("        MMO Engine Swarm Load Tester Initialized           ");
    println!("============================================================");
    println!("Target Gateway    : {}", cli.gateway_addr);
    println!("Bot Count         : {}", cli.bots);
    println!("Duration          : {} seconds", cli.duration_secs);
    println!("Tick Rate         : {} Hz", cli.tick_rate);
    println!("Max Bot Speed     : {} m/s", cli.max_speed);
    println!("------------------------------------------------------------");

    let config = SwarmConfig {
        gateway_addr: cli.gateway_addr,
        bot_count: cli.bots,
        duration: Duration::from_secs(cli.duration_secs),
        tick_rate_hz: cli.tick_rate,
        ping_interval: Duration::from_millis(1000),
        stagger_delay: Duration::from_millis(cli.stagger_ms),
        bounds: WorldBounds::default_arena(),
        max_bot_speed: cli.max_speed,
    };

    let manager = SwarmManager::new(config);
    let snapshot = manager.run().await;

    println!("\n============================================================");
    println!("                   LOAD TEST SUMMARY                        ");
    println!("============================================================");
    println!("Duration           : {:.2} s", snapshot.duration_secs);
    println!("Packets Sent       : {} ({:.1} pkts/s)", snapshot.total_packets_sent, snapshot.send_rate_pps);
    println!("Packets Received   : {} ({:.1} pkts/s)", snapshot.total_packets_received, snapshot.recv_rate_pps);
    println!("Snapback Rejections: {}", snapshot.snapbacks);
    println!("Connection Errors  : {}", snapshot.connection_errors);
    println!("------------------------------------------------------------");
    println!("Latency (RTT):");
    println!("  Min              : {:.2} ms", snapshot.min_rtt_ms);
    println!("  p50 (Median)     : {:.2} ms", snapshot.p50_rtt_ms);
    println!("  p95              : {:.2} ms", snapshot.p95_rtt_ms);
    println!("  p99              : {:.2} ms", snapshot.p99_rtt_ms);
    println!("  Max              : {:.2} ms", snapshot.max_rtt_ms);
    println!("============================================================");

    Ok(())
}
