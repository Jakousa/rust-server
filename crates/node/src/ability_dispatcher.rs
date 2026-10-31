//! Outbound Cross-Partition Ability Dispatcher.
//!
//! Provides [`CrossNodeDispatcher`] managing peer node endpoints and asynchronous gRPC
//! ability invocations with non-blocking compensation pipelines on tick loops.

use std::collections::HashMap;
use std::time::Duration;

use mmo_core::action::{resolve_boundary_sphere_aoe, AbilityDefinition, CooldownTracker};
use mmo_core::guid::Guid;
use mmo_core::AttributeSet;
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::{BatchRemoteAbilityRequest, RemoteAbilityRequest, Vector3};

use crate::network::GrpcClientPool;
use crate::sim::{AbilityCompensation, TickSimulation};

/// Default network timeout for remote ability dispatch ($100\text{ms}$).
pub const DEFAULT_DISPATCH_TIMEOUT: Duration = Duration::from_millis(100);

/// Outcome of attempting to cast an ability across partition boundaries.
#[derive(Debug, Clone, PartialEq)]
pub enum CastOutcome {
    /// Ability was executed immediately on a local entity.
    LocalExecuted,
    /// Ability target was identified as a boundary ghost and forwarded to the owning node.
    RemoteDispatched { target_node_id: u32 },
}

/// Manages outbound cross-node ability forwarding, peer endpoints, and compensation tasks.
#[derive(Debug, Clone)]
pub struct CrossNodeDispatcher {
    pub local_node_id: u32,
    pub peer_endpoints: HashMap<u32, String>,
    pub timeout: Duration,
    pub pool: GrpcClientPool,
}

impl CrossNodeDispatcher {
    /// Creates a new `CrossNodeDispatcher` for `local_node_id` with default timeout.
    pub fn new(local_node_id: u32) -> Self {
        Self {
            local_node_id,
            peer_endpoints: HashMap::new(),
            timeout: DEFAULT_DISPATCH_TIMEOUT,
            pool: GrpcClientPool::default(),
        }
    }

    /// Creates a new `CrossNodeDispatcher` with pre-configured peer endpoints.
    pub fn with_peers(local_node_id: u32, peer_endpoints: HashMap<u32, String>) -> Self {
        Self {
            local_node_id,
            peer_endpoints,
            timeout: DEFAULT_DISPATCH_TIMEOUT,
            pool: GrpcClientPool::default(),
        }
    }

    /// Attaches a custom [`GrpcClientPool`] to this dispatcher.
    pub fn with_pool(mut self, pool: GrpcClientPool) -> Self {
        self.pool = pool;
        self
    }

    /// Registers or updates a peer simulation node's endpoint.
    pub fn add_peer(&mut self, node_id: u32, endpoint: impl Into<String>) {
        self.peer_endpoints.insert(node_id, endpoint.into());
    }

    /// Removes a peer node endpoint.
    pub fn remove_peer(&mut self, node_id: u32) -> Option<String> {
        self.peer_endpoints.remove(&node_id)
    }

    /// Returns the endpoint URL registered for a peer node.
    pub fn peer_endpoint(&self, node_id: u32) -> Option<&str> {
        self.peer_endpoints.get(&node_id).map(|s| s.as_str())
    }

    /// Asynchronously forwards a targeted ability request to a remote peer node.
    ///
    /// If the remote node rejects the cast, returns an error status, or times out,
    /// an [`AbilityCompensation`] is pushed into `compensation_tx` to refund resources
    /// and reset cooldowns without blocking the simulation tick.
    pub fn dispatch_remote(
        &self,
        req: RemoteAbilityRequest,
        endpoint: String,
        compensation_tx: tokio::sync::mpsc::Sender<AbilityCompensation>,
        resource_refund: Option<(String, f32)>,
    ) {
        let timeout = self.timeout;
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let res = tokio::time::timeout(timeout, async {
                let channel = pool.get_channel(&endpoint).map_err(|e| e.to_string())?;
                let mut client = NodeServiceClient::new(channel);
                let resp = client
                    .dispatch_ability(req)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(resp.into_inner())
            })
            .await;

            let should_compensate = match res {
                Ok(Ok(resp)) => !resp.success,
                Ok(Err(_)) => true,
                Err(_) => true, // Timeout
            };

            if should_compensate {
                let comp = AbilityCompensation {
                    caster_guid: Guid::from_raw(req.caster_guid),
                    ability_id: req.ability_id,
                    resource_name: resource_refund.as_ref().map(|(n, _)| n.clone()),
                    resource_amount: resource_refund.as_ref().map(|(_, a)| *a).unwrap_or(0.0),
                };
                if compensation_tx.try_send(comp).is_err() {
                    metrics::counter!("mmo_node_ingress_dropped_total", "queue" => "compensation")
                        .increment(1);
                }
            }
        });
    }

    /// Asynchronously forwards a batched ability request for boundary multi-target AoE.
    pub fn dispatch_remote_batch(
        &self,
        req: BatchRemoteAbilityRequest,
        endpoint: String,
        compensation_tx: tokio::sync::mpsc::Sender<AbilityCompensation>,
        resource_refund: Option<(String, f32)>,
    ) {
        let timeout = self.timeout;
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let res = tokio::time::timeout(timeout, async {
                let channel = pool.get_channel(&endpoint).map_err(|e| e.to_string())?;
                let mut client = NodeServiceClient::new(channel);
                let resp = client
                    .dispatch_batch_ability(req.clone())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(resp.into_inner())
            })
            .await;

            let all_failed = match res {
                Ok(Ok(resp)) => resp.results.iter().all(|r| !r.success),
                Ok(Err(_)) => true,
                Err(_) => true,
            };

            if all_failed {
                let comp = AbilityCompensation {
                    caster_guid: Guid::from_raw(req.caster_guid),
                    ability_id: req.ability_id,
                    resource_name: resource_refund.as_ref().map(|(n, _)| n.clone()),
                    resource_amount: resource_refund.as_ref().map(|(_, a)| *a).unwrap_or(0.0),
                };
                if compensation_tx.try_send(comp).is_err() {
                    metrics::counter!("mmo_node_ingress_dropped_total", "queue" => "compensation")
                        .increment(1);
                }
            }
        });
    }
}

/// Executes Phase 1 of cross-partition ability casting on the local simulation node.
///
/// If `target_guid` is owned locally, applies damage or healing directly.
/// If `target_guid` is in `ghost_store`, validates caster cooldown & resources, applies
/// proactive cooldown + GCD locally, and invokes [`CrossNodeDispatcher::dispatch_remote`].
pub fn cast_cross_partition_ability(
    sim: &mut TickSimulation,
    dispatcher: &CrossNodeDispatcher,
    caster_guid: Guid,
    ability: &AbilityDefinition,
    target_guid: Guid,
    damage: u32,
    healing: u32,
) -> Result<CastOutcome, &'static str> {
    // 1. Check if target is local
    if sim.entity_store.contains_guid(target_guid) {
        let target_ent = sim
            .entity_store
            .get_by_guid(target_guid)
            .ok_or("Target not found")?;
        if target_ent.is_incapacitated() {
            return Err("Target entity is dead");
        }

        let caster = sim
            .entity_store
            .get_mut_by_guid(caster_guid)
            .ok_or("Caster not found")?;

        if let Some(cd) = caster.get_component::<CooldownTracker>() {
            if cd.is_on_cooldown(ability.id) {
                return Err("Ability is currently on cooldown");
            }
        }

        if let Some((ref res_name, cost)) = ability.resource_cost {
            if let Some(attrs) = caster.get_component_mut::<AttributeSet>() {
                let current = attrs
                    .get_current(res_name)
                    .ok_or("Resource attribute not found on caster")?;
                if current < cost {
                    return Err("Insufficient resource to activate ability");
                }
                attrs.modify_current(res_name, -cost)?;
            }
        }

        if let Some(cd) = caster.get_component_mut::<CooldownTracker>() {
            cd.start_cooldown(ability.id, ability.cooldown_sec, ability.gcd_sec);
        }

        // Apply effect directly to local target
        let target_ent = sim
            .entity_store
            .get_mut_by_guid(target_guid)
            .ok_or("Target not found")?;
        crate::combat_pipeline::apply_combat_effect(
            target_ent,
            target_guid,
            Some(caster_guid),
            damage,
            healing,
            &mut sim.event_queue,
        )?;

        return Ok(CastOutcome::LocalExecuted);
    }

    // 2. Check if target is a replicated boundary ghost
    let (target_node_id, endpoint) = if let Some(ghost) = sim.ghost_store.get(target_guid) {
        let node_id = ghost.source_node_id;
        let ep = dispatcher
            .peer_endpoint(node_id)
            .ok_or("No gRPC endpoint configured for peer node owning target ghost")?
            .to_string();
        (node_id, ep)
    } else {
        return Err("Target entity not found in local store or ghost store");
    };

    // 3. Phase 1 (Local): Caster validation, resource deduction & proactive cooldown
    let caster = sim
        .entity_store
        .get_mut_by_guid(caster_guid)
        .ok_or("Caster not found in entity store")?;

    if let Some(cd) = caster.get_component::<CooldownTracker>() {
        if cd.is_on_cooldown(ability.id) {
            return Err("Ability is currently on cooldown");
        }
    }

    if let Some((ref res_name, cost)) = ability.resource_cost {
        if let Some(attrs) = caster.get_component_mut::<AttributeSet>() {
            let current = attrs
                .get_current(res_name)
                .ok_or("Resource attribute not found on caster")?;
            if current < cost {
                return Err("Insufficient resource to activate ability");
            }
            attrs.modify_current(res_name, -cost)?;
        }
    }

    // Proactive cooldown and GCD application prevents boundary dance exploits
    if let Some(cd) = caster.get_component_mut::<CooldownTracker>() {
        cd.start_cooldown(ability.id, ability.cooldown_sec, ability.gcd_sec);
    }

    let caster_pos = caster.position;

    // 4. Construct remote ability request
    let req = RemoteAbilityRequest {
        ability_id: ability.id,
        caster_guid: caster_guid.raw(),
        target_guid: target_guid.raw(),
        source_node_id: sim.node_id as u32,
        target_node_id,
        caster_position: Some(Vector3 {
            x: caster_pos.0,
            y: caster_pos.1,
            z: caster_pos.2,
        }),
        max_range: ability.range,
        damage,
        healing,
        resource_cost: ability
            .resource_cost
            .as_ref()
            .map(|(_, c)| *c as u32)
            .unwrap_or(0),
        cast_id: sim.tick_counter,
    };

    // 5. Asynchronous dispatch with non-blocking compensation on failure
    dispatcher.dispatch_remote(
        req,
        endpoint,
        sim.ability_compensation_sender(),
        ability.resource_cost.clone(),
    );

    Ok(CastOutcome::RemoteDispatched { target_node_id })
}

/// Result of casting a boundary-crossing AoE ability.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryAoeOutcome {
    pub local_hit_count: usize,
    pub remote_dispatched_nodes: Vec<u32>,
}

/// Executes a boundary-crossing Area-of-Effect sphere ability.
///
/// 1. Queries local entities and replicated ghosts within `radius` of `center`.
/// 2. Commits local entity damage/healing immediately within the simulation tick.
/// 3. Groups remote targets by owning node ID and asynchronously dispatches batches in parallel.
/// 4. Ensures zero duplicate hits across partition boundaries.
#[allow(clippy::too_many_arguments)]
pub fn cast_boundary_sphere_aoe(
    sim: &mut TickSimulation,
    dispatcher: &CrossNodeDispatcher,
    caster_guid: Guid,
    ability: &AbilityDefinition,
    center: mmo_core::Vec3,
    radius: f32,
    max_targets: usize,
    damage_per_target: u32,
    healing_per_target: u32,
) -> Result<BoundaryAoeOutcome, &'static str> {
    // 1. Gather all candidates: local entities in octree + replicated ghosts in ghost_store
    let mut candidates = Vec::new();

    let center_pos = (center.x, center.y, center.z);

    // Local entities (spatial index query instead of a full entity_store scan)
    for guid in sim.octree.query_radius(center_pos, radius) {
        if guid == caster_guid {
            continue; // Caster excluded from harmful cleave
        }
        let Some(entity) = sim.entity_store.get_by_guid(guid) else {
            continue;
        };
        let is_alive = !entity.is_incapacitated();
        candidates.push(mmo_core::spatial::SpatialTargetCandidate::new(
            entity.guid,
            sim.node_id as u32,
            mmo_core::Vec3::new(entity.position.0, entity.position.1, entity.position.2),
            is_alive,
            false,
        ));
    }

    // Ghost replicas
    for ghost in sim.ghost_store.query_radius(center_pos, radius) {
        // Zero duplication check: ensure ghost isn't accidentally in entity_store
        if sim.entity_store.contains_guid(ghost.guid) {
            continue;
        }
        candidates.push(mmo_core::spatial::SpatialTargetCandidate::new(
            ghost.guid,
            ghost.source_node_id,
            mmo_core::Vec3::new(ghost.position.0, ghost.position.1, ghost.position.2),
            true, // Assume alive unless known otherwise
            false,
        ));
    }

    // 2. Resolve candidates & partition by node
    let targeting = resolve_boundary_sphere_aoe(
        &candidates,
        center,
        radius,
        max_targets,
        true,
        sim.node_id as u32,
    );

    let has_any_targets =
        !targeting.local_targets.is_empty() || !targeting.remote_batches.is_empty();
    if !has_any_targets {
        return Err("No valid targets in radius");
    }

    // 3. Phase 1 (Local): Caster validation, resource deduction & proactive cooldown
    let caster = sim
        .entity_store
        .get_mut_by_guid(caster_guid)
        .ok_or("Caster not found in entity store")?;

    if let Some(cd) = caster.get_component::<CooldownTracker>() {
        if cd.is_on_cooldown(ability.id) {
            return Err("Ability is currently on cooldown");
        }
    }

    if let Some((ref res_name, cost)) = ability.resource_cost {
        if let Some(attrs) = caster.get_component_mut::<AttributeSet>() {
            let current = attrs
                .get_current(res_name)
                .ok_or("Resource attribute not found on caster")?;
            if current < cost {
                return Err("Insufficient resource to activate ability");
            }
            attrs.modify_current(res_name, -cost)?;
        }
    }

    if let Some(cd) = caster.get_component_mut::<CooldownTracker>() {
        cd.start_cooldown(ability.id, ability.cooldown_sec, ability.gcd_sec);
    }

    let caster_pos = caster.position;

    // 4. Commit local hits immediately
    let mut local_hit_count = 0;
    for local_cand in &targeting.local_targets {
        if let Some(target_ent) = sim.entity_store.get_mut_by_guid(local_cand.guid) {
            if crate::combat_pipeline::apply_combat_effect(
                target_ent,
                local_cand.guid,
                Some(caster_guid),
                damage_per_target,
                healing_per_target,
                &mut sim.event_queue,
            )
            .is_ok()
            {
                local_hit_count += 1;
            }
        }
    }

    // 5. Concurrently dispatch remote batches to peer nodes
    let mut remote_nodes = Vec::new();
    for (node_id, remote_candidates) in &targeting.remote_batches {
        let endpoint = match dispatcher.peer_endpoint(*node_id) {
            Some(ep) => ep.to_string(),
            None => continue,
        };

        let req = BatchRemoteAbilityRequest {
            ability_id: ability.id,
            caster_guid: caster_guid.raw(),
            source_node_id: sim.node_id as u32,
            target_node_id: *node_id,
            caster_position: Some(Vector3 {
                x: caster_pos.0,
                y: caster_pos.1,
                z: caster_pos.2,
            }),
            max_range: ability.range,
            damage_per_target,
            healing_per_target,
            target_guids: remote_candidates.iter().map(|c| c.guid.raw()).collect(),
            cast_id: sim.tick_counter,
        };

        let refund = if local_hit_count == 0 {
            ability.resource_cost.clone()
        } else {
            None
        };

        dispatcher.dispatch_remote_batch(req, endpoint, sim.ability_compensation_sender(), refund);
        remote_nodes.push(*node_id);
    }

    Ok(BoundaryAoeOutcome {
        local_hit_count,
        remote_dispatched_nodes: remote_nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::GhostRecord;
    use mmo_core::guid::EntityType;
    use mmo_core::{ActionInterruptFlags, BoundingBox3D};

    fn test_bounds() -> BoundingBox3D {
        BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0))
    }

    #[test]
    fn test_dispatcher_peer_management() {
        let mut dispatcher = CrossNodeDispatcher::new(1);
        assert_eq!(dispatcher.peer_endpoint(2), None);

        dispatcher.add_peer(2, "127.0.0.1:50052");
        assert_eq!(dispatcher.peer_endpoint(2), Some("127.0.0.1:50052"));

        assert_eq!(
            dispatcher.remove_peer(2),
            Some("127.0.0.1:50052".to_string())
        );
        assert_eq!(dispatcher.peer_endpoint(2), None);
    }

    #[tokio::test]
    async fn test_cast_cross_partition_remote_and_compensation() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let caster = Guid::new(EntityType::Player, 1, 10);
        let ghost_target = Guid::new(EntityType::Player, 2, 20);

        // Spawn caster with energy and cooldown tracker
        sim.spawn_entity(caster, (0.0, 0.0, 0.0));
        let ent = sim.entity_store.get_mut_by_guid(caster).unwrap();
        let mut attrs = AttributeSet::new();
        attrs.add_attribute("energy", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());

        // Insert ghost for target
        sim.ghost_store.insert_or_update(GhostRecord {
            guid: ghost_target,
            position: (10.0, 0.0, 0.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: Vec::new(),
            last_tick: 1,
            source_node_id: 2,
        });

        let mut dispatcher = CrossNodeDispatcher::new(1);
        // Point node 2 to a dead port to simulate network failure and trigger compensation
        dispatcher.add_peer(2, "127.0.0.1:59998");

        let ability = AbilityDefinition {
            id: 7,
            name: "Shadow Bolt".into(),
            cast_time_sec: 0.0,
            cooldown_sec: 8.0,
            gcd_sec: 1.5,
            range: 25.0,
            resource_cost: Some(("energy".into(), 30.0)),
            interrupt_flags: ActionInterruptFlags::default(),
        };

        let outcome = cast_cross_partition_ability(
            &mut sim,
            &dispatcher,
            caster,
            &ability,
            ghost_target,
            50,
            0,
        );

        assert_eq!(
            outcome,
            Ok(CastOutcome::RemoteDispatched { target_node_id: 2 })
        );

        // Verify proactive resource deduction and cooldown application
        {
            let ent = sim.entity_store.get_by_guid(caster).unwrap();
            let energy = ent
                .get_component::<AttributeSet>()
                .unwrap()
                .get_current("energy")
                .unwrap();
            assert_eq!(energy, 70.0);

            let cd = ent.get_component::<CooldownTracker>().unwrap();
            assert!(cd.is_on_cooldown(7));
        }

        // Wait a short duration for the background task to fail against the dead port
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Run simulation step to drain the compensation queue
        sim.step(0.05);

        // Verify resource refunded and cooldown reset!
        {
            let ent = sim.entity_store.get_by_guid(caster).unwrap();
            let energy = ent
                .get_component::<AttributeSet>()
                .unwrap()
                .get_current("energy")
                .unwrap();
            assert_eq!(energy, 100.0);

            let cd = ent.get_component::<CooldownTracker>().unwrap();
            assert!(!cd.is_on_cooldown(7));
        }
    }

    #[tokio::test]
    async fn test_cast_boundary_sphere_aoe_multitarget() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let caster = Guid::new(EntityType::Player, 1, 100);
        let local_target = Guid::new(EntityType::Player, 1, 101);
        let ghost_target = Guid::new(EntityType::Player, 2, 201);

        sim.spawn_entity(caster, (0.0, 0.0, 0.0));
        let ent_caster = sim.entity_store.get_mut_by_guid(caster).unwrap();
        let mut attrs = AttributeSet::new();
        attrs.add_attribute("energy", 100.0, 0.0, 100.0, 0.0);
        ent_caster.add_component(attrs);
        ent_caster.add_component(CooldownTracker::new());

        sim.spawn_entity(local_target, (2.0, 0.0, 0.0));
        let ent_local = sim.entity_store.get_mut_by_guid(local_target).unwrap();
        let mut target_attrs = AttributeSet::new();
        target_attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);
        ent_local.add_component(target_attrs);

        sim.ghost_store.insert_or_update(GhostRecord {
            guid: ghost_target,
            position: (4.0, 0.0, 0.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: Vec::new(),
            last_tick: 1,
            source_node_id: 2,
        });

        let mut dispatcher = CrossNodeDispatcher::new(1);
        dispatcher.add_peer(2, "127.0.0.1:59997");

        let ability = AbilityDefinition {
            id: 8,
            name: "Whirlwind".into(),
            cast_time_sec: 0.0,
            cooldown_sec: 6.0,
            gcd_sec: 1.5,
            range: 10.0,
            resource_cost: Some(("energy".into(), 25.0)),
            interrupt_flags: ActionInterruptFlags::default(),
        };

        let outcome = cast_boundary_sphere_aoe(
            &mut sim,
            &dispatcher,
            caster,
            &ability,
            mmo_core::Vec3::new(0.0, 0.0, 0.0),
            10.0,
            5,
            40,
            0,
        )
        .expect("boundary aoe success");

        assert_eq!(outcome.local_hit_count, 1);
        assert_eq!(outcome.remote_dispatched_nodes, vec![2]);

        // Local target took damage immediately
        let ent_local = sim.entity_store.get_by_guid(local_target).unwrap();
        let hp = ent_local
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("health")
            .unwrap();
        assert_eq!(hp, 60.0);

        // Caster paid energy
        let ent_c = sim.entity_store.get_by_guid(caster).unwrap();
        let energy = ent_c
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("energy")
            .unwrap();
        assert_eq!(energy, 75.0);
    }

    #[tokio::test]
    async fn test_cast_local_target_dead_rejected_without_resource_loss() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let caster = Guid::new(EntityType::Player, 1, 10);
        let dead_target = Guid::new(EntityType::Player, 1, 20);

        sim.spawn_entity(caster, (0.0, 0.0, 0.0));
        let ent = sim.entity_store.get_mut_by_guid(caster).unwrap();
        let mut attrs = AttributeSet::new();
        attrs.add_attribute("energy", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());

        sim.spawn_entity(dead_target, (5.0, 0.0, 0.0));
        let ent_t = sim.entity_store.get_mut_by_guid(dead_target).unwrap();
        ent_t.state_flags |= mmo_core::entity::state_flags::INCAPACITATED;

        let dispatcher = CrossNodeDispatcher::new(1);
        let ability = AbilityDefinition {
            id: 1,
            name: "Fireball".into(),
            cast_time_sec: 0.0,
            cooldown_sec: 5.0,
            gcd_sec: 1.0,
            range: 20.0,
            resource_cost: Some(("energy".into(), 25.0)),
            interrupt_flags: ActionInterruptFlags::default(),
        };

        let res = cast_cross_partition_ability(
            &mut sim,
            &dispatcher,
            caster,
            &ability,
            dead_target,
            50,
            0,
        );

        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "Target entity is dead");

        // Caster energy should NOT be deducted
        let ent_c = sim.entity_store.get_by_guid(caster).unwrap();
        let energy = ent_c
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("energy")
            .unwrap();
        assert_eq!(energy, 100.0);

        // Cooldown should NOT be started
        let cd = ent_c.get_component::<CooldownTracker>().unwrap();
        assert!(!cd.is_on_cooldown(1));
    }

    #[test]
    fn test_cast_local_target_mitigation_and_death_event() {
        let mut sim = TickSimulation::new(1, test_bounds());
        let caster = Guid::new(EntityType::Player, 1, 10);
        let target = Guid::new(EntityType::Player, 1, 30);

        sim.spawn_entity(caster, (0.0, 0.0, 0.0));
        let ent = sim.entity_store.get_mut_by_guid(caster).unwrap();
        ent.add_component(CooldownTracker::new());

        sim.spawn_entity(target, (5.0, 0.0, 0.0));
        let ent_t = sim.entity_store.get_mut_by_guid(target).unwrap();
        let mut target_attrs = AttributeSet::new();
        target_attrs.add_attribute("health", 20.0, 0.0, 20.0, 0.0);
        target_attrs.add_attribute("armor", 50.0, 0.0, 1000.0, 0.0); // 33.3% mitigation
        ent_t.add_component(target_attrs);

        let dispatcher = CrossNodeDispatcher::new(1);
        let ability = AbilityDefinition {
            id: 2,
            name: "Strike".into(),
            cast_time_sec: 0.0,
            cooldown_sec: 0.0,
            gcd_sec: 0.0,
            range: 10.0,
            resource_cost: None,
            interrupt_flags: ActionInterruptFlags::default(),
        };

        // 30 base damage -> 20 mitigated damage -> exactly 0 HP -> dies
        let outcome =
            cast_cross_partition_ability(&mut sim, &dispatcher, caster, &ability, target, 30, 0)
                .expect("cast succeeds");

        assert_eq!(outcome, CastOutcome::LocalExecuted);

        let ent_t = sim.entity_store.get_by_guid(target).unwrap();
        assert!(ent_t.is_incapacitated());

        // Verify EntityIncapacitatedEvent in sim.event_queue
        let death_events = sim
            .event_queue
            .drain::<mmo_core::plugin::EntityIncapacitatedEvent>();
        assert_eq!(death_events.len(), 1);
        assert_eq!(death_events[0].entity, target);
        assert_eq!(death_events[0].instigator, Some(caster));
    }
}
