use crate::metrics::{MetricsAggregator, MetricsSnapshot};
use crate::trajectory::{BehaviorProfile, TrajectoryController, WorldBounds};
use bytes::BytesMut;
use mmo_core::guid::{EntityType, Guid};
use mmo_schema::client::{ClientPacket, InteractionPacket, InteractionType, MovementPacket, PingPacket, Vec3};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::sleep;

/// Configuration parameters for the bot swarm run.
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    pub gateway_addr: String,
    pub bot_count: usize,
    pub duration: Duration,
    pub tick_rate_hz: u64,
    pub ping_interval: Duration,
    pub stagger_delay: Duration,
    pub bounds: WorldBounds,
    pub max_bot_speed: f32,
    pub profile: BehaviorProfile,
    pub hotspot_center: Vec3,
    pub hotspot_radius: f32,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            gateway_addr: "127.0.0.1:8080".to_string(),
            bot_count: 50,
            duration: Duration::from_secs(10),
            tick_rate_hz: 20,
            ping_interval: Duration::from_millis(1000),
            stagger_delay: Duration::from_millis(5),
            bounds: WorldBounds::default_arena(),
            max_bot_speed: 15.0,
            profile: BehaviorProfile::RandomWalk,
            hotspot_center: Vec3::ZERO,
            hotspot_radius: 50.0,
        }
    }
}

/// Swarm manager running the multi-agent load simulation.
pub struct SwarmManager {
    config: SwarmConfig,
    metrics: Arc<MetricsAggregator>,
}

impl SwarmManager {
    pub fn new(config: SwarmConfig) -> Self {
        Self {
            config,
            metrics: Arc::new(MetricsAggregator::new()),
        }
    }

    pub fn metrics(&self) -> Arc<MetricsAggregator> {
        Arc::clone(&self.metrics)
    }

    /// Executes the swarm simulation until duration elapses.
    pub async fn run(&self) -> MetricsSnapshot {
        let (shutdown_tx, _) = broadcast::channel(1);
        let mut bot_handles = Vec::with_capacity(self.config.bot_count);

        println!(
            "Spawning {} bots targeting gateway at {} (duration: {:?})...",
            self.config.bot_count, self.config.gateway_addr, self.config.duration
        );

        let dt = 1.0 / self.config.tick_rate_hz as f32;
        let tick_interval = Duration::from_secs_f32(dt);

        for i in 0..self.config.bot_count {
            let guid = Guid::new(EntityType::Player, 1, (i + 1) as u32);
            let profile = self.config.profile;
            let mut traj = TrajectoryController::with_profile(
                Vec3::new(0.0, 0.0, 0.0),
                self.config.max_bot_speed,
                self.config.bounds,
                self.config.profile,
                self.config.hotspot_center,
                self.config.hotspot_radius,
            );
            let target_addr = self.config.gateway_addr.clone();
            let metrics = Arc::clone(&self.metrics);
            let mut shutdown_rx = shutdown_tx.subscribe();
            let ping_interval = self.config.ping_interval;

            // Stagger connection startup
            if self.config.stagger_delay > Duration::ZERO {
                sleep(self.config.stagger_delay).await;
            }

            let handle = tokio::spawn(async move {
                let stream = match TcpStream::connect(&target_addr).await {
                    Ok(s) => {
                        let _ = s.set_nodelay(true);
                        s
                    }
                    Err(e) => {
                        eprintln!("[Bot {:?}] Connection failed: {}", guid, e);
                        metrics.record_connection_error();
                        return;
                    }
                };

                let (mut reader, mut writer) = stream.into_split();
                let mut read_buf = BytesMut::with_capacity(4096);
                let mut tick_timer = tokio::time::interval(tick_interval);
                tick_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                let mut next_ping = Instant::now();
                let mut last_ping: Option<(u64, Instant)> = None;
                let start_time = Instant::now();
                let mut seq: u32 = 0;

                loop {
                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            break;
                        }
                        read_res = reader.read_buf(&mut read_buf) => {
                            match read_res {
                                Ok(0) => {
                                    metrics.record_connection_error();
                                    break;
                                }
                                Ok(_) => {
                                    while let Ok(Some(packet)) = ClientPacket::decode_frame(&mut read_buf) {
                                        metrics.record_packet_received();
                                        match packet {
                                            ClientPacket::Pong(pong) => {
                                                if let Some((ts, sent_time)) = last_ping {
                                                    if ts == pong.client_timestamp {
                                                        metrics.record_rtt_ms(sent_time.elapsed().as_secs_f64() * 1000.0);
                                                        last_ping = None;
                                                    }
                                                }
                                            }
                                            ClientPacket::MovementCorrection(corr) if corr.entity_id == guid.raw() => {
                                                metrics.record_snapback();
                                                traj.position = corr.corrected_position;
                                                traj.velocity = Vec3::ZERO;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Err(_) => {
                                    metrics.record_connection_error();
                                    break;
                                }
                            }
                        }
                        _ = tick_timer.tick() => {
                            let (pos, vel) = traj.step(dt);
                            let now_ms = start_time.elapsed().as_millis() as u64;
                            seq += 1;

                            // Send movement
                            let move_pkt = ClientPacket::Movement(MovementPacket {
                                entity_id: guid.raw(),
                                position: pos,
                                velocity: vel,
                                yaw: 0.0,
                                pitch: 0.0,
                                timestamp: now_ms,
                                sequence: seq,
                            });
                            let encoded = move_pkt.encode();
                            if writer.write_all(&encoded).await.is_ok() {
                                metrics.record_packet_sent();
                            } else {
                                metrics.record_connection_error();
                                break;
                            }

                            // If profile is CombatInteraction, send an interaction packet periodically
                            if profile == BehaviorProfile::CombatInteraction && seq.is_multiple_of(20) {
                                let interact_pkt = ClientPacket::Interaction(InteractionPacket {
                                    source_guid: guid.raw(),
                                    target_guid: 0,
                                    interaction_type: InteractionType::Attack,
                                    action_id: 1,
                                    payload: bytes::Bytes::new(),
                                });
                                let encoded_interact = interact_pkt.encode();
                                if writer.write_all(&encoded_interact).await.is_ok() {
                                    metrics.record_packet_sent();
                                }
                            }

                            // Send ping periodically
                            if Instant::now() >= next_ping {
                                next_ping = Instant::now() + ping_interval;
                                let ping_pkt = ClientPacket::Ping(PingPacket { timestamp: now_ms });
                                let encoded_ping = ping_pkt.encode();
                                if writer.write_all(&encoded_ping).await.is_ok() {
                                    metrics.record_packet_sent();
                                    last_ping = Some((now_ms, Instant::now()));
                                }
                            }
                        }
                    }
                }
            });

            bot_handles.push(handle);
        }

        println!(
            "All bots dispatched. Running simulation for {:?}...",
            self.config.duration
        );
        sleep(self.config.duration).await;

        // Signal shutdown
        let _ = shutdown_tx.send(());

        for handle in bot_handles {
            let _ = handle.await;
        }

        self.metrics.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use mmo_schema::client::ClientPacket;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_swarm_manager_lifecycle() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway_addr = listener.local_addr().unwrap().to_string();

        let server_task = tokio::spawn(async move {
            let mut sockets = Vec::new();
            for _ in 0..5 {
                let (socket, _) = listener.accept().await.unwrap();
                sockets.push(socket);
            }

            for mut s in sockets {
                let mut buf = BytesMut::with_capacity(1024);
                if let Ok(n) = s.read_buf(&mut buf).await {
                    if n > 0 {
                        if let Ok(Some(ClientPacket::Ping(ping))) =
                            ClientPacket::decode_frame(&mut buf)
                        {
                            let pong = ClientPacket::Pong(mmo_schema::client::PongPacket {
                                client_timestamp: ping.timestamp,
                                server_timestamp: 1000,
                            });
                            let out = pong.encode();
                            let _ = s.write_all(&out).await;
                        }
                    }
                }
            }
        });

        let config = SwarmConfig {
            gateway_addr,
            bot_count: 5,
            duration: Duration::from_millis(500),
            tick_rate_hz: 20,
            ping_interval: Duration::from_millis(100),
            stagger_delay: Duration::from_millis(1),
            bounds: WorldBounds::default_arena(),
            max_bot_speed: 10.0,
            ..Default::default()
        };

        let manager = SwarmManager::new(config);
        let snapshot = manager.run().await;

        assert!(snapshot.total_packets_sent > 0);
        server_task.await.unwrap();
    }
}
