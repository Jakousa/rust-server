use crate::metrics::{MetricsAggregator, MetricsSnapshot};
use crate::session::{BotSession, SessionError};
use crate::trajectory::{BehaviorProfile, TrajectoryController, WorldBounds};
use bytes::BytesMut;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mmo_core::guid::{EntityType, Guid};
use mmo_schema::client::{
    ClientAckPacket, ClientPacket, InteractionPacket, InteractionType, MovementPacket, PingPacket,
    Vec3,
};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::broadcast;
use tokio::time::sleep;

#[derive(Serialize)]
struct BotJwtClaims {
    sub: String,
    exp: u64,
    nbf: u64,
    iat: u64,
    guid: u64,
    roles: Vec<String>,
}

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
    /// Optional explicit authentication token sent in `AuthRequestPacket` by each bot.
    /// When `None` and `auth_secret` is `Some`, a per-bot HS256 JWT is minted.
    /// When both are `None`, a development token (`dev-bot-{guid}`) is sent.
    pub auth_token: Option<String>,
    /// Optional HS256 shared secret used to sign unique per-bot JWTs when `auth_token` is `None`.
    pub auth_secret: Option<String>,
    /// Maximum duration each bot waits for `AuthResponsePacket` before failing.
    pub auth_timeout: Duration,
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
            auth_token: None,
            auth_secret: None,
            auth_timeout: Duration::from_secs(5),
        }
    }
}

impl SwarmConfig {
    /// Sets a static authentication token to send in each bot's `AuthRequestPacket`.
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Sets an HS256 shared secret used to sign unique per-bot JWTs.
    pub fn with_auth_secret(mut self, secret: impl Into<String>) -> Self {
        self.auth_secret = Some(secret.into());
        self
    }

    /// Sets the authentication handshake timeout per bot.
    pub fn with_auth_timeout(mut self, timeout: Duration) -> Self {
        self.auth_timeout = timeout;
        self
    }

    fn auth_token_for_bot(&self, guid: Guid) -> String {
        if let Some(ref token) = self.auth_token {
            return token.clone();
        }
        if let Some(ref secret) = self.auth_secret {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let claims = BotJwtClaims {
                sub: guid.raw().to_string(),
                exp: now + 3600,
                nbf: now.saturating_sub(10),
                iat: now,
                guid: guid.raw(),
                roles: vec!["player".to_string()],
            };
            if let Ok(jwt) = encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(secret.as_bytes()),
            ) {
                return jwt;
            }
        }
        format!("dev-bot-{}", guid.raw())
    }
}

async fn process_bot_frames(
    read_buf: &mut BytesMut,
    writer: &mut OwnedWriteHalf,
    metrics: &MetricsAggregator,
    session_id: u64,
    guid: Guid,
    traj: &mut TrajectoryController,
    last_ping: &mut Option<(u64, Instant)>,
) {
    while let Ok(Some(packet)) = ClientPacket::decode_frame(read_buf) {
        metrics.record_packet_received();
        match packet {
            ClientPacket::Pong(pong) => {
                if let Some((ts, sent_time)) = *last_ping {
                    if ts == pong.client_timestamp {
                        metrics.record_rtt_ms(sent_time.elapsed().as_secs_f64() * 1000.0);
                        *last_ping = None;
                    }
                }
            }
            ClientPacket::MovementCorrection(corr) if corr.entity_id == guid.raw() => {
                metrics.record_snapback();
                traj.position = corr.corrected_position;
                traj.velocity = Vec3::ZERO;
            }
            ClientPacket::EntityDeltaBatch(batch) => {
                let ack_pkt = ClientPacket::ClientAck(ClientAckPacket {
                    session_id,
                    observed_tick: batch.server_tick,
                    ack_mask: 0,
                });
                let encoded_ack = ack_pkt.encode();
                if writer.write_all(&encoded_ack).await.is_ok() {
                    metrics.record_packet_sent();
                }
            }
            _ => {}
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

        let tick_rate_hz = self.config.tick_rate_hz.max(1);
        let dt = 1.0 / tick_rate_hz as f32;
        let tick_interval = Duration::from_secs_f32(dt);

        for i in 0..self.config.bot_count {
            let initial_guid = Guid::new(EntityType::Player, 1, (i + 1) as u32);
            let auth_token = self.config.auth_token_for_bot(initial_guid);
            let auth_timeout = self.config.auth_timeout;
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
                let mut session = BotSession::new(initial_guid, Vec3::ZERO);
                if let Err(e) = session.connect(&target_addr).await {
                    eprintln!("[Bot {:?}] Connection failed: {}", initial_guid, e);
                    metrics.record_connection_error();
                    return;
                }

                let auth_result = tokio::select! {
                    _ = shutdown_rx.recv() => return,
                    res = session.authenticate(auth_token, auth_timeout) => res,
                };
                match auth_result {
                    Ok(_) => {
                        metrics.record_packet_sent();
                        metrics.record_packet_received();
                    }
                    Err(e) => {
                        match &e {
                            SessionError::AuthRejected { .. } => {
                                metrics.record_packet_sent();
                                metrics.record_packet_received();
                            }
                            SessionError::AuthTimeout => {
                                metrics.record_packet_sent();
                            }
                            _ => {}
                        }
                        eprintln!("[Bot {:?}] Authentication failed: {}", initial_guid, e);
                        metrics.record_auth_failure();
                        metrics.record_connection_error();
                        return;
                    }
                }

                let session_id = session.session_id;
                let guid = session.guid;
                let Some((stream, mut read_buf)) = session.into_parts() else {
                    metrics.record_connection_error();
                    return;
                };

                let (mut reader, mut writer) = stream.into_split();
                let mut tick_timer = tokio::time::interval(tick_interval);
                tick_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                let mut next_ping = Instant::now();
                let mut last_ping: Option<(u64, Instant)> = None;
                let start_time = Instant::now();
                let mut seq: u32 = 0;

                if !read_buf.is_empty() {
                    process_bot_frames(
                        &mut read_buf,
                        &mut writer,
                        &metrics,
                        session_id,
                        guid,
                        &mut traj,
                        &mut last_ping,
                    )
                    .await;
                }

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
                                    process_bot_frames(
                                        &mut read_buf,
                                        &mut writer,
                                        &metrics,
                                        session_id,
                                        guid,
                                        &mut traj,
                                        &mut last_ping,
                                    )
                                    .await;
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
    use mmo_gateway::auth::EdgeAuthenticator;
    use mmo_gateway::router::SessionRouter;
    use mmo_gateway::server::GatewayServer;
    use mmo_schema::client::{AuthResponsePacket, ClientPacket, EntityDeltaBatchPacket};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_swarm_manager_lifecycle() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway_addr = listener.local_addr().unwrap().to_string();

        let server_task = tokio::spawn(async move {
            let mut conn_tasks = Vec::with_capacity(5);
            for idx in 0..5u64 {
                let (mut s, _) = listener.accept().await.unwrap();
                conn_tasks.push(tokio::spawn(async move {
                    let assigned_session_id = 100 + idx;
                    let assigned_guid = Guid::new(EntityType::Player, 1, (idx + 1) as u32);
                    let mut buf = BytesMut::with_capacity(1024);

                    // 1. Expect AuthRequest first
                    let n = s.read_buf(&mut buf).await.unwrap();
                    assert!(n > 0);
                    match ClientPacket::decode_frame(&mut buf).unwrap().unwrap() {
                        ClientPacket::AuthRequest(auth) => {
                            assert!(!auth.token.is_empty());
                            let resp = ClientPacket::AuthResponse(AuthResponsePacket {
                                success: true,
                                error_code: 0,
                                session_id: assigned_session_id,
                                player_guid: assigned_guid.raw(),
                                assigned_node: "127.0.0.1:50052".to_string(),
                                error_message: String::new(),
                            });
                            s.write_all(&resp.encode()).await.unwrap();
                        }
                        other => panic!("Expected AuthRequest first, got {other:?}"),
                    }

                    // 2. Push EntityDeltaBatch and wait for ClientAck carrying assigned_session_id
                    let batch = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
                        server_tick: 42,
                        baseline_tick: 41,
                        despawns: Vec::new(),
                        deltas: Vec::new(),
                    });
                    s.write_all(&batch.encode()).await.unwrap();

                    let deadline = Instant::now() + Duration::from_secs(2);
                    let mut captured_ack = None;
                    while Instant::now() < deadline && captured_ack.is_none() {
                        let read_res =
                            tokio::time::timeout(Duration::from_millis(200), s.read_buf(&mut buf))
                                .await;
                        if let Ok(Ok(m)) = read_res {
                            if m == 0 {
                                break;
                            }
                            while let Ok(Some(pkt)) = ClientPacket::decode_frame(&mut buf) {
                                match pkt {
                                    ClientPacket::ClientAck(ack) => {
                                        captured_ack = Some(ack.session_id);
                                    }
                                    ClientPacket::Ping(ping) => {
                                        let pong =
                                            ClientPacket::Pong(mmo_schema::client::PongPacket {
                                                client_timestamp: ping.timestamp,
                                                server_timestamp: 1000,
                                            });
                                        let _ = s.write_all(&pong.encode()).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    assert_eq!(captured_ack, Some(assigned_session_id));
                    captured_ack.unwrap()
                }));
            }

            let mut ack_session_ids = Vec::with_capacity(conn_tasks.len());
            for task in conn_tasks {
                ack_session_ids.push(task.await.unwrap());
            }
            ack_session_ids.sort_unstable();
            ack_session_ids
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
        assert_eq!(snapshot.auth_failures, 0);
        let ack_ids = server_task.await.unwrap();
        assert_eq!(ack_ids, vec![100, 101, 102, 103, 104]);
    }

    #[tokio::test]
    async fn test_swarm_manager_live_gateway_jwt_auth_and_client_ack() {
        let secret = "swarm_live_gateway_jwt_secret_123456";
        let auth = Arc::new(EdgeAuthenticator::new_hs256(secret.as_bytes(), None, None));
        let router = Arc::new(SessionRouter::new());
        let (packet_tx, mut packet_rx) = mpsc::channel(256);

        let gateway = Arc::new(
            GatewayServer::with_packet_channel(Arc::clone(&router), packet_tx)
                .with_authenticator(auth)
                .with_default_node_address("127.0.0.1:50052"),
        );

        let listener = gateway.bind("127.0.0.1:0").await.unwrap();
        let gateway_addr = listener.local_addr().unwrap().to_string();

        let gw_accept = Arc::clone(&gateway);
        let accept_task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let gw = Arc::clone(&gw_accept);
                let session_id = gw.next_session_id();
                tokio::spawn(async move {
                    let _ = gw.handle_connection(socket, session_id).await;
                });
            }
        });

        let config = SwarmConfig {
            gateway_addr,
            bot_count: 3,
            duration: Duration::from_millis(450),
            tick_rate_hz: 20,
            ping_interval: Duration::from_millis(150),
            stagger_delay: Duration::from_millis(1),
            ..Default::default()
        }
        .with_auth_secret(secret);

        let manager = SwarmManager::new(config);
        let gw_broadcast = Arc::clone(&gateway);
        let broadcast_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let batch = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
                server_tick: 77,
                baseline_tick: 76,
                despawns: Vec::new(),
                deltas: Vec::new(),
            });
            let _ = gw_broadcast.broadcast_packet(&batch).await;
        });

        let collect_task = tokio::spawn(async move {
            let mut saw_movement = false;
            let mut ack_session_ids = Vec::new();
            while let Some(routed) = packet_rx.recv().await {
                match routed.packet {
                    ClientPacket::Movement(_) => saw_movement = true,
                    ClientPacket::ClientAck(ack) => {
                        assert_ne!(ack.session_id, 0);
                        assert_eq!(ack.session_id, routed.session_id);
                        assert_eq!(ack.observed_tick, 77);
                        ack_session_ids.push(ack.session_id);
                    }
                    _ => {}
                }
            }
            (saw_movement, ack_session_ids)
        });

        let snapshot = manager.run().await;
        broadcast_task.await.unwrap();
        accept_task.abort();
        drop(gateway);
        let (saw_movement, ack_session_ids) = collect_task.await.unwrap();

        assert_eq!(snapshot.auth_failures, 0);
        assert!(saw_movement, "Authenticated bots must forward Movement");
        assert_eq!(
            ack_session_ids.len(),
            3,
            "All 3 bots must send ClientAck with their server-assigned session_id"
        );
    }

    #[tokio::test]
    async fn test_swarm_manager_handles_auth_failure_gracefully() {
        let secret = "swarm_rejection_test_secret_987654";
        let auth = Arc::new(EdgeAuthenticator::new_hs256(secret.as_bytes(), None, None));
        let router = Arc::new(SessionRouter::new());
        let (packet_tx, mut packet_rx) = mpsc::channel(32);

        let gateway = Arc::new(
            GatewayServer::with_packet_channel(Arc::clone(&router), packet_tx)
                .with_authenticator(auth)
                .with_default_node_address("127.0.0.1:50052"),
        );

        let listener = gateway.bind("127.0.0.1:0").await.unwrap();
        let gateway_addr = listener.local_addr().unwrap().to_string();

        let gw_accept = Arc::clone(&gateway);
        let accept_task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let gw = Arc::clone(&gw_accept);
                let session_id = gw.next_session_id();
                tokio::spawn(async move {
                    let _ = gw.handle_connection(socket, session_id).await;
                });
            }
        });

        let config = SwarmConfig {
            gateway_addr,
            bot_count: 3,
            duration: Duration::from_millis(200),
            stagger_delay: Duration::from_millis(1),
            ..Default::default()
        }
        .with_auth_token("invalid.jwt.token");

        let manager = SwarmManager::new(config);
        let snapshot = manager.run().await;

        assert_eq!(snapshot.auth_failures, 3);
        assert_eq!(snapshot.connection_errors, 3);
        assert_eq!(router.session_count(), 0);
        assert!(
            packet_rx.try_recv().is_err(),
            "Rejected bots must not forward gameplay packets"
        );
        accept_task.abort();
    }
}
