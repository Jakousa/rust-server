//! Declarative Scenario-Driven E2E Test Runner.
//!
//! Executes multi-bot orchestration scenarios described via JSON manifests against
//! Edge Gateways and authoritative simulation clusters.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mmo_core::guid::{EntityType, Guid};
use mmo_schema::client::{
    AuthRequestPacket, ClientPacket, MovementPacket, SocialMessagePacket, Vec3,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::session::{BotSession, SessionError};

/// High-level declarative scenario specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioManifest {
    /// Scenario name or identifier.
    pub name: String,
    /// Edge Gateway target TCP address (e.g., "127.0.0.1:8080").
    pub gateway_addr: String,
    /// Optional SLA threshold for p99 round-trip time in milliseconds.
    #[serde(default)]
    pub max_p99_rtt_ms: Option<f64>,
    /// Chronological list of steps to execute.
    pub steps: Vec<ScenarioStep>,
}

/// Individual action step within a scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ScenarioStep {
    /// Connect a bot session to the target gateway.
    Connect { bot_id: u32 },
    /// Send an authentication request token and assert result.
    Authenticate {
        bot_id: u32,
        token: String,
        expect_success: bool,
    },
    /// Dispatch client movement update.
    Move {
        bot_id: u32,
        position: [f32; 3],
        velocity: [f32; 3],
        yaw: f32,
    },
    /// Dispatch ping packet to measure RTT latency.
    Ping { bot_id: u32 },
    /// Pause execution for the specified duration in milliseconds while draining incoming packets.
    WaitMs { duration_ms: u64 },
    /// Assert that a bot has observed a specific entity in its Area of Interest.
    AssertKnownEntity {
        bot_id: u32,
        target_guid: u64,
        max_distance: Option<f32>,
    },
    /// Dispatch a chat or social broadcast packet.
    Chat {
        bot_id: u32,
        channel: String,
        message: String,
    },
    /// Terminate a bot session connection.
    Disconnect { bot_id: u32 },
}

/// Summary report produced upon scenario completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioReport {
    /// Name of executed scenario.
    pub scenario_name: String,
    /// Total number of steps executed.
    pub steps_executed: usize,
    /// Total wall-clock execution duration in milliseconds.
    pub total_duration_ms: u64,
    /// Median (50th percentile) round-trip time in milliseconds.
    pub p50_rtt_ms: f64,
    /// 99th percentile round-trip time in milliseconds.
    pub p99_rtt_ms: f64,
    /// Captured errors or empty on clean pass.
    pub errors: Vec<String>,
}

/// Errors occurring during scenario execution.
#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    #[error("Assertion failed: {0}")]
    AssertionFailed(String),

    #[error("SLA violation: p99 RTT {actual:.2}ms exceeds threshold {max:.2}ms")]
    SlaViolation { actual: f64, max: f64 },

    #[error("Bot session {0} not found")]
    BotNotFound(u32),
}

/// Orchestrator for executing declarative scenario manifests.
pub struct ScenarioRunner;

impl ScenarioRunner {
    /// Loads a scenario manifest from disk and executes it.
    pub async fn run_from_file(path: impl AsRef<Path>) -> Result<ScenarioReport, ScenarioError> {
        let content = tokio::fs::read_to_string(path).await?;
        let manifest: ScenarioManifest = serde_json::from_str(&content)?;
        Self::run(&manifest).await
    }

    /// Executes the specified scenario manifest against target gateway.
    pub async fn run(manifest: &ScenarioManifest) -> Result<ScenarioReport, ScenarioError> {
        let start_time = Instant::now();
        let mut sessions: HashMap<u32, BotSession> = HashMap::new();
        let mut rtt_samples: Vec<f64> = Vec::new();
        let mut steps_executed = 0;

        for step in &manifest.steps {
            steps_executed += 1;
            match step {
                ScenarioStep::Connect { bot_id } => {
                    let mut session =
                        BotSession::new(Guid::new(EntityType::Player, 1, *bot_id), Vec3::ZERO);
                    session.connect(&manifest.gateway_addr).await?;
                    sessions.insert(*bot_id, session);
                }
                ScenarioStep::Authenticate {
                    bot_id,
                    token,
                    expect_success,
                } => {
                    let session = sessions
                        .get_mut(bot_id)
                        .ok_or(ScenarioError::BotNotFound(*bot_id))?;
                    let auth_packet = ClientPacket::AuthRequest(AuthRequestPacket {
                        token: token.clone(),
                        client_version: 1,
                    });
                    session.send_packet(&auth_packet).await?;

                    let deadline = Instant::now() + Duration::from_secs(3);
                    let mut got_response = false;
                    while Instant::now() < deadline {
                        match session
                            .poll_incoming_timeout(Duration::from_millis(50))
                            .await
                        {
                            Ok((packets, rtt)) => {
                                if let Some(val) = rtt {
                                    rtt_samples.push(val);
                                }
                                for pkt in packets {
                                    if let ClientPacket::AuthResponse(resp) = pkt {
                                        got_response = true;
                                        if *expect_success {
                                            if !resp.success {
                                                return Err(ScenarioError::AssertionFailed(format!(
                                                    "Bot {} authentication expected success, got error {}: {}",
                                                    bot_id, resp.error_code, resp.error_message
                                                )));
                                            }
                                            session.session_id = resp.session_id;
                                            if resp.player_guid != 0 {
                                                session.guid = Guid::from_raw(resp.player_guid);
                                            }
                                        } else if resp.success {
                                            return Err(ScenarioError::AssertionFailed(format!(
                                                "Bot {} authentication expected failure, but succeeded",
                                                bot_id
                                            )));
                                        }
                                        break;
                                    }
                                }
                                if got_response {
                                    break;
                                }
                            }
                            Err(SessionError::Io(_)) if !*expect_success => {
                                got_response = true;
                                break;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    if !got_response {
                        return Err(ScenarioError::AssertionFailed(format!(
                            "Bot {} authentication timed out waiting for server response",
                            bot_id
                        )));
                    }
                }
                ScenarioStep::Move {
                    bot_id,
                    position,
                    velocity,
                    yaw,
                } => {
                    let session = sessions
                        .get_mut(bot_id)
                        .ok_or(ScenarioError::BotNotFound(*bot_id))?;
                    session.current_position = Vec3::new(position[0], position[1], position[2]);
                    session.current_velocity = Vec3::new(velocity[0], velocity[1], velocity[2]);
                    session.sequence = session.sequence.wrapping_add(1);

                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    let pkt = ClientPacket::Movement(MovementPacket {
                        entity_id: session.guid.raw(),
                        position: session.current_position,
                        velocity: session.current_velocity,
                        yaw: *yaw,
                        pitch: 0.0,
                        timestamp: now_ms,
                        sequence: session.sequence,
                    });
                    session.send_packet(&pkt).await?;

                    if let Ok((_, Some(rtt))) = session
                        .poll_incoming_timeout(Duration::from_millis(10))
                        .await
                    {
                        rtt_samples.push(rtt);
                    }
                }
                ScenarioStep::Ping { bot_id } => {
                    let session = sessions
                        .get_mut(bot_id)
                        .ok_or(ScenarioError::BotNotFound(*bot_id))?;
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    session.send_ping(now_ms).await?;

                    let deadline = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < deadline {
                        let (_, rtt) = session
                            .poll_incoming_timeout(Duration::from_millis(50))
                            .await?;
                        if let Some(val) = rtt {
                            rtt_samples.push(val);
                            break;
                        }
                    }
                }
                ScenarioStep::WaitMs { duration_ms } => {
                    let wait_deadline = Instant::now() + Duration::from_millis(*duration_ms);
                    while Instant::now() < wait_deadline {
                        let remaining = wait_deadline.saturating_duration_since(Instant::now());
                        let slice = remaining.min(Duration::from_millis(20));
                        tokio::time::sleep(slice).await;

                        for session in sessions.values_mut() {
                            if session.is_connected() {
                                if let Ok((_, Some(rtt))) = session
                                    .poll_incoming_timeout(Duration::from_millis(2))
                                    .await
                                {
                                    rtt_samples.push(rtt);
                                }
                            }
                        }
                    }
                }
                ScenarioStep::AssertKnownEntity {
                    bot_id,
                    target_guid,
                    max_distance,
                } => {
                    let session = sessions
                        .get_mut(bot_id)
                        .ok_or(ScenarioError::BotNotFound(*bot_id))?;

                    // Poll briefly for any pending updates to arrive
                    let deadline = Instant::now() + Duration::from_millis(300);
                    let mut found_pos = session.known_entities.get(target_guid).copied();
                    while found_pos.is_none() && Instant::now() < deadline {
                        let _ = session
                            .poll_incoming_timeout(Duration::from_millis(50))
                            .await;
                        found_pos = session.known_entities.get(target_guid).copied();
                    }

                    let target_pos = found_pos.ok_or_else(|| {
                        ScenarioError::AssertionFailed(format!(
                            "Bot {} does not know entity {}",
                            bot_id, target_guid
                        ))
                    })?;

                    if let Some(max_dist) = max_distance {
                        let dx = session.current_position.x - target_pos.x;
                        let dy = session.current_position.y - target_pos.y;
                        let dz = session.current_position.z - target_pos.z;
                        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                        if dist > *max_dist {
                            return Err(ScenarioError::AssertionFailed(format!(
                                "Bot {} entity {} distance {:.2} exceeds max_distance {:.2}",
                                bot_id, target_guid, dist, max_dist
                            )));
                        }
                    }
                }
                ScenarioStep::Chat {
                    bot_id,
                    channel,
                    message,
                } => {
                    let session = sessions
                        .get_mut(bot_id)
                        .ok_or(ScenarioError::BotNotFound(*bot_id))?;
                    let channel_type = match channel.to_lowercase().as_str() {
                        "global" | "world" => 1,
                        "area" | "local" => 2,
                        "party" => 3,
                        "guild" => 4,
                        "whisper" => 5,
                        _ => channel.parse::<u8>().unwrap_or(1),
                    };
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    let pkt = ClientPacket::SocialMessage(SocialMessagePacket {
                        channel_type,
                        target_id: 0,
                        sender_guid: session.guid.raw(),
                        sender_name: format!("bot_{}", bot_id),
                        content: message.clone(),
                        timestamp: now_ms,
                    });
                    session.send_packet(&pkt).await?;
                }
                ScenarioStep::Disconnect { bot_id } => {
                    let session = sessions
                        .get_mut(bot_id)
                        .ok_or(ScenarioError::BotNotFound(*bot_id))?;
                    session.disconnect();
                }
            }
        }

        let total_duration_ms = start_time.elapsed().as_millis() as u64;
        let (p50_rtt_ms, p99_rtt_ms) = if rtt_samples.is_empty() {
            (0.0, 0.0)
        } else {
            rtt_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let p50_idx = ((rtt_samples.len() as f64) * 0.50) as usize;
            let p99_idx = (((rtt_samples.len() as f64) * 0.99) as usize).min(rtt_samples.len() - 1);
            (rtt_samples[p50_idx], rtt_samples[p99_idx])
        };

        if let Some(max_p99) = manifest.max_p99_rtt_ms {
            if p99_rtt_ms > max_p99 {
                return Err(ScenarioError::SlaViolation {
                    actual: p99_rtt_ms,
                    max: max_p99,
                });
            }
        }

        Ok(ScenarioReport {
            scenario_name: manifest.name.clone(),
            steps_executed,
            total_duration_ms,
            p50_rtt_ms,
            p99_rtt_ms,
            errors: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scenario_manifest_json_roundtrip_all_step_variants() {
        let manifest = ScenarioManifest {
            name: "comprehensive_e2e".to_string(),
            gateway_addr: "127.0.0.1:8080".to_string(),
            max_p99_rtt_ms: Some(150.0),
            steps: vec![
                ScenarioStep::Connect { bot_id: 1 },
                ScenarioStep::Authenticate {
                    bot_id: 1,
                    token: "valid_jwt_token".to_string(),
                    expect_success: true,
                },
                ScenarioStep::Move {
                    bot_id: 1,
                    position: [10.0, 0.0, -5.5],
                    velocity: [1.0, 0.0, 0.0],
                    yaw: 1.57,
                },
                ScenarioStep::Ping { bot_id: 1 },
                ScenarioStep::WaitMs { duration_ms: 100 },
                ScenarioStep::AssertKnownEntity {
                    bot_id: 1,
                    target_guid: 42,
                    max_distance: Some(30.0),
                },
                ScenarioStep::Chat {
                    bot_id: 1,
                    channel: "chat:global".to_string(),
                    message: "Hello cluster".to_string(),
                },
                ScenarioStep::Disconnect { bot_id: 1 },
            ],
        };

        let json_str = serde_json::to_string_pretty(&manifest).expect("serialization succeeds");
        assert!(json_str.contains(r#""action": "connect""#));
        assert!(json_str.contains(r#""action": "authenticate""#));
        assert!(json_str.contains(r#""action": "move""#));
        assert!(json_str.contains(r#""action": "ping""#));
        assert!(json_str.contains(r#""action": "wait_ms""#));
        assert!(json_str.contains(r#""action": "assert_known_entity""#));
        assert!(json_str.contains(r#""action": "chat""#));
        assert!(json_str.contains(r#""action": "disconnect""#));

        let parsed: ScenarioManifest =
            serde_json::from_str(&json_str).expect("deserialization succeeds");
        assert_eq!(parsed.name, "comprehensive_e2e");
        assert_eq!(parsed.gateway_addr, "127.0.0.1:8080");
        assert_eq!(parsed.max_p99_rtt_ms, Some(150.0));
        assert_eq!(parsed.steps.len(), 8);
    }

    #[test]
    fn test_scenario_report_serialization() {
        let report = ScenarioReport {
            scenario_name: "bench_run".to_string(),
            steps_executed: 10,
            total_duration_ms: 500,
            p50_rtt_ms: 12.5,
            p99_rtt_ms: 35.0,
            errors: vec!["minor glitch".to_string()],
        };
        let serialized = serde_json::to_string(&report).unwrap();
        let deserialized: ScenarioReport = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.scenario_name, "bench_run");
        assert_eq!(deserialized.steps_executed, 10);
        assert_eq!(deserialized.errors.len(), 1);
    }
}
