//! Standalone NPC Navigation and Kinematic Steering Plugin.
//!
//! Provides waypoint following, linear velocity steering, arrival detection,
//! and spawn leashing boundary enforcement.

use serde::{Deserialize, Serialize};

use mmo_core::{GameplayPlugin, Guid, PluginContext, TickContext, TickSystem};

/// Loop behavior when traversing a sequence of waypoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaypointLoopMode {
    /// Wraps back to the first waypoint after reaching the last.
    Loop,
    /// Reverses traversal direction upon reaching an endpoint.
    PingPong,
    /// Halts navigation upon reaching the final waypoint.
    Once,
}

/// Dynamic component defining a sequence of 3D waypoints for entity patrol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaypointComponent {
    pub waypoints: Vec<(f32, f32, f32)>,
    pub current_index: usize,
    pub loop_mode: WaypointLoopMode,
    pub ping_pong_reverse: bool,
    pub is_paused: bool,
}

impl WaypointComponent {
    pub fn new(waypoints: Vec<(f32, f32, f32)>, loop_mode: WaypointLoopMode) -> Self {
        Self {
            waypoints,
            current_index: 0,
            loop_mode,
            ping_pong_reverse: false,
            is_paused: false,
        }
    }

    /// Returns the current active target waypoint, if any exist.
    pub fn current_waypoint(&self) -> Option<(f32, f32, f32)> {
        self.waypoints.get(self.current_index).copied()
    }

    /// Advances to the next waypoint in accordance with `loop_mode`.
    pub fn advance(&mut self) {
        if self.waypoints.is_empty() {
            return;
        }

        match self.loop_mode {
            WaypointLoopMode::Loop => {
                self.current_index = (self.current_index + 1) % self.waypoints.len();
            }
            WaypointLoopMode::PingPong => {
                if !self.ping_pong_reverse {
                    if self.current_index + 1 >= self.waypoints.len() {
                        self.ping_pong_reverse = true;
                        if self.current_index > 0 {
                            self.current_index -= 1;
                        }
                    } else {
                        self.current_index += 1;
                    }
                } else if self.current_index == 0 {
                    self.ping_pong_reverse = false;
                    if self.waypoints.len() > 1 {
                        self.current_index += 1;
                    }
                } else {
                    self.current_index -= 1;
                }
            }
            WaypointLoopMode::Once => {
                if self.current_index + 1 < self.waypoints.len() {
                    self.current_index += 1;
                } else {
                    self.is_paused = true;
                }
            }
        }
    }
}

/// Dynamic component representing kinematic movement capabilities and destinations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavAgentComponent {
    pub target_position: Option<(f32, f32, f32)>,
    pub speed: f32,
    pub arrival_tolerance: f32,
    pub has_arrived: bool,
}

impl NavAgentComponent {
    pub fn new(speed: f32, arrival_tolerance: f32) -> Self {
        Self {
            target_position: None,
            speed,
            arrival_tolerance,
            has_arrived: false,
        }
    }

    pub fn with_target(mut self, target: (f32, f32, f32)) -> Self {
        self.target_position = Some(target);
        self
    }
}

/// Dynamic component bounding entity movement to a leash radius from spawn origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeashComponent {
    pub spawn_origin: (f32, f32, f32),
    pub max_leash_radius: f32,
    pub is_leashed_out: bool,
}

impl LeashComponent {
    pub fn new(spawn_origin: (f32, f32, f32), max_leash_radius: f32) -> Self {
        Self {
            spawn_origin,
            max_leash_radius,
            is_leashed_out: false,
        }
    }

    /// Checks if a position exceeds the leash radius.
    pub fn is_outside_leash(&self, pos: (f32, f32, f32)) -> bool {
        let dx = pos.0 - self.spawn_origin.0;
        let dy = pos.1 - self.spawn_origin.1;
        let dz = pos.2 - self.spawn_origin.2;
        let dist_sq = dx * dx + dy * dy + dz * dz;
        dist_sq > self.max_leash_radius * self.max_leash_radius
    }
}

/// Batch tick system that moves entities toward targets and checks leashing boundaries.
#[derive(Default)]
pub struct NavSteeringSystem;

impl TickSystem for NavSteeringSystem {
    fn name(&self) -> &'static str {
        "NavSteeringSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        // Collect entity moves to apply in octree
        let mut updates: Vec<(Guid, (f32, f32, f32), (f32, f32, f32))> = Vec::new();

        for (_key, entity) in ctx.entities.iter_mut() {
            let old_pos = entity.position;

            // 1. Resolve target from WaypointComponent if NavAgent target is unset
            let wp_target = if let Some(waypoints) = entity.get_component::<WaypointComponent>() {
                if !waypoints.is_paused {
                    waypoints.current_waypoint()
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(target) = wp_target {
                if let Some(agent) = entity.get_component_mut::<NavAgentComponent>() {
                    if agent.target_position.is_none() {
                        agent.target_position = Some(target);
                        agent.has_arrived = false;
                    }
                }
            }

            // 3. Steer toward target_position if present
            let mut should_advance_wp = false;
            let mut new_pos_opt = None;
            if let Some(agent) = entity.get_component_mut::<NavAgentComponent>() {
                if let Some(target) = agent.target_position {
                    let dx = target.0 - old_pos.0;
                    let dy = target.1 - old_pos.1;
                    let dz = target.2 - old_pos.2;
                    let dist = (dx * dx + dy * dy + dz * dz).sqrt();

                    if dist <= agent.arrival_tolerance {
                        agent.has_arrived = true;
                        agent.target_position = None;
                        should_advance_wp = true;
                    } else {
                        let step = (agent.speed * ctx.dt_sec).min(dist);
                        let ratio = step / dist;
                        let new_pos = (
                            old_pos.0 + dx * ratio,
                            old_pos.1 + dy * ratio,
                            old_pos.2 + dz * ratio,
                        );
                        new_pos_opt = Some(new_pos);

                        if (dist - step) <= agent.arrival_tolerance {
                            agent.has_arrived = true;
                            agent.target_position = None;
                            should_advance_wp = true;
                        } else {
                            agent.has_arrived = false;
                        }
                    }
                }
            }

            if let Some(new_pos) = new_pos_opt {
                entity.position = new_pos;
                updates.push((entity.guid, old_pos, new_pos));
            }

            // 4. Check leash bounds on current position
            let current_pos = entity.position;
            if let Some(leash) = entity.get_component_mut::<LeashComponent>() {
                if leash.is_outside_leash(current_pos) {
                    leash.is_leashed_out = true;
                }
            }

            // Advance waypoints if arrived
            if should_advance_wp {
                let next_target = if let Some(waypoints) = entity.get_component_mut::<WaypointComponent>() {
                    if !waypoints.is_paused {
                        waypoints.advance();
                        waypoints.current_waypoint()
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(next) = next_target {
                    if let Some(agent) = entity.get_component_mut::<NavAgentComponent>() {
                        agent.target_position = Some(next);
                        agent.has_arrived = false;
                    }
                }
            }
        }

        // Keep Octree updated with new positions
        for (guid, old_pos, new_pos) in updates {
            ctx.octree.remove(guid, old_pos);
            ctx.octree.insert(guid, new_pos);
        }
    }
}

/// Standalone plugin registering NPC navigation and steering components.
pub struct NpcMovementPlugin;

impl GameplayPlugin for NpcMovementPlugin {
    fn id(&self) -> &'static str {
        "npc_movement"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<WaypointComponent>("npc_waypoints");
        ctx.codecs.register::<NavAgentComponent>("npc_nav_agent");
        ctx.codecs.register::<LeashComponent>("npc_leash");

        ctx.tick_systems.push(Box::new(NavSteeringSystem));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::{BoundingBox3D, EntityStore, EventQueue, EntityType, Octree};

    #[test]
    fn test_waypoint_component_modes() {
        let waypoints = vec![(0.0, 0.0, 0.0), (10.0, 0.0, 0.0), (20.0, 0.0, 0.0)];

        // Loop mode
        let mut loop_wp = WaypointComponent::new(waypoints.clone(), WaypointLoopMode::Loop);
        assert_eq!(loop_wp.current_index, 0);
        loop_wp.advance();
        assert_eq!(loop_wp.current_index, 1);
        loop_wp.advance();
        assert_eq!(loop_wp.current_index, 2);
        loop_wp.advance();
        assert_eq!(loop_wp.current_index, 0); // Wraps

        // Ping-pong mode
        let mut ping_wp = WaypointComponent::new(waypoints.clone(), WaypointLoopMode::PingPong);
        assert_eq!(ping_wp.current_index, 0);
        ping_wp.advance();
        assert_eq!(ping_wp.current_index, 1);
        ping_wp.advance();
        assert_eq!(ping_wp.current_index, 2);
        ping_wp.advance();
        assert_eq!(ping_wp.current_index, 1); // Reverses
        ping_wp.advance();
        assert_eq!(ping_wp.current_index, 0);
        ping_wp.advance();
        assert_eq!(ping_wp.current_index, 1); // Bounces back
    }

    #[test]
    fn test_nav_steering_system_stepping() {
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));
        let mut events = EventQueue::new();

        let npc_guid = Guid::new(EntityType::Npc, 1, 100);
        let key = store.spawn(npc_guid, (0.0, 0.0, 0.0));
        octree.insert(npc_guid, (0.0, 0.0, 0.0));

        let agent = NavAgentComponent::new(10.0, 0.5) // 10 units/sec, arrival tolerance 0.5m
            .with_target((10.0, 0.0, 0.0));
        let leash = LeashComponent::new((0.0, 0.0, 0.0), 15.0);

        {
            let ent = store.get_mut(key).unwrap();
            ent.add_component(agent);
            ent.add_component(leash);
        }

        let mut steering_system = NavSteeringSystem;

        // Step 0.5 seconds: speed 10.0 * 0.5s = 5.0 units
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.5,
                tick: 1,
            };
            steering_system.update(&mut ctx);
        }

        let ent = store.get(key).unwrap();
        assert!((ent.position.0 - 5.0).abs() < 1e-4);
        assert!(!ent.get_component::<NavAgentComponent>().unwrap().has_arrived);
        assert!(!ent.get_component::<LeashComponent>().unwrap().is_leashed_out);

        // Step another 0.5 seconds: reaches 10.0, dist = 0 <= 0.5 tolerance -> arrived!
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.5,
                tick: 2,
            };
            steering_system.update(&mut ctx);
        }

        let ent = store.get(key).unwrap();
        assert!((ent.position.0 - 10.0).abs() < 1e-4);
        assert!(ent.get_component::<NavAgentComponent>().unwrap().has_arrived);
        assert!(!ent.get_component::<LeashComponent>().unwrap().is_leashed_out);

        // Step outside leash: target at (20.0, 0.0, 0.0)
        store
            .get_mut(key)
            .unwrap()
            .get_component_mut::<NavAgentComponent>()
            .unwrap()
            .target_position = Some((20.0, 0.0, 0.0));

        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 1.0, // Moves 10 units: 10 + 10 = 20.0 (> 15 leash)
                tick: 3,
            };
            steering_system.update(&mut ctx);
        }

        let ent = store.get(key).unwrap();
        assert!(ent.get_component::<LeashComponent>().unwrap().is_leashed_out);
    }
}
