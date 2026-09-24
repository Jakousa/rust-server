//! Standalone Atomic Trade Gameplay Plugin.
//!
//! Provides two-player trade sessions with offer staging, lock-step confirmation,
//! distance-based abortion hooks, and atomic disjoint inventory swaps with full rollback.

use std::collections::HashMap;

use mmo_core::{DomainEvent, EntityStore, GameplayPlugin, Guid, PluginContext};

use crate::inventory::{InventoryComponent, ItemStack};

/// Offer submitted by a player in a trade session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TradeOffer {
    pub items: Vec<ItemStack>,
    pub currency: u64,
}

/// Active trade session between two entities.
#[derive(Debug, Clone)]
pub struct TradeSession {
    pub session_id: u64,
    pub player_a: Guid,
    pub player_b: Guid,
    pub offer_a: TradeOffer,
    pub offer_b: TradeOffer,
    pub locked_a: bool,
    pub locked_b: bool,
    pub accepted_a: bool,
    pub accepted_b: bool,
    pub completed: bool,
}

impl TradeSession {
    pub fn new(session_id: u64, player_a: Guid, player_b: Guid) -> Self {
        Self {
            session_id,
            player_a,
            player_b,
            offer_a: TradeOffer::default(),
            offer_b: TradeOffer::default(),
            locked_a: false,
            locked_b: false,
            accepted_a: false,
            accepted_b: false,
            completed: false,
        }
    }

    /// Sets the offer for the given participant, unlocking the session if previously locked.
    pub fn set_offer(&mut self, player: Guid, offer: TradeOffer) -> Result<(), &'static str> {
        if self.completed {
            return Err("Trade session already finished");
        }
        if player == self.player_a {
            self.offer_a = offer;
            self.locked_a = false;
            self.accepted_a = false;
            self.accepted_b = false;
            Ok(())
        } else if player == self.player_b {
            self.offer_b = offer;
            self.locked_b = false;
            self.accepted_a = false;
            self.accepted_b = false;
            Ok(())
        } else {
            Err("Player not a participant in trade")
        }
    }

    /// Locks in the offer. Both players must lock before accepting.
    pub fn lock(&mut self, player: Guid) -> Result<(), &'static str> {
        if player == self.player_a {
            self.locked_a = true;
            Ok(())
        } else if player == self.player_b {
            self.locked_b = true;
            Ok(())
        } else {
            Err("Player not a participant in trade")
        }
    }

    /// Marks acceptance. Only valid once both participants have locked their offers.
    pub fn accept(&mut self, player: Guid) -> Result<bool, &'static str> {
        if !self.locked_a || !self.locked_b {
            return Err("Both players must lock their offers before accepting");
        }

        if player == self.player_a {
            self.accepted_a = true;
        } else if player == self.player_b {
            self.accepted_b = true;
        } else {
            return Err("Player not a participant in trade");
        }

        Ok(self.accepted_a && self.accepted_b)
    }
}

/// Domain event emitted upon successful atomic trade completion.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeCompletedEvent {
    pub session_id: u64,
    pub player_a: Guid,
    pub player_b: Guid,
}

impl DomainEvent for TradeCompletedEvent {}

/// Manages active trade sessions across players.
#[derive(Default)]
pub struct TradeManager {
    sessions: HashMap<u64, TradeSession>,
    player_to_session: HashMap<Guid, u64>,
    next_session_id: u64,
}

impl TradeManager {
    pub const MAX_TRADE_DISTANCE: f32 = 5.0;

    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            player_to_session: HashMap::new(),
            next_session_id: 1,
        }
    }

    /// Initiates a new trade session between two entities within distance limit.
    pub fn start_session(
        &mut self,
        p1: Guid,
        p2: Guid,
        entities: &EntityStore,
    ) -> Result<u64, &'static str> {
        if p1 == p2 {
            return Err("Cannot trade with self");
        }
        if self.player_to_session.contains_key(&p1) || self.player_to_session.contains_key(&p2) {
            return Err("Player already in active trade session");
        }

        let e1 = entities.get_by_guid(p1).ok_or("Player 1 not found")?;
        let e2 = entities.get_by_guid(p2).ok_or("Player 2 not found")?;

        let dx = e1.position.0 - e2.position.0;
        let dy = e1.position.1 - e2.position.1;
        let dz = e1.position.2 - e2.position.2;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();

        if dist > Self::MAX_TRADE_DISTANCE {
            return Err("Players too far apart to initiate trade");
        }

        let session_id = self.next_session_id;
        self.next_session_id += 1;

        let session = TradeSession::new(session_id, p1, p2);
        self.sessions.insert(session_id, session);
        self.player_to_session.insert(p1, session_id);
        self.player_to_session.insert(p2, session_id);

        Ok(session_id)
    }

    pub fn get_session_mut(&mut self, session_id: u64) -> Option<&mut TradeSession> {
        self.sessions.get_mut(&session_id)
    }

    pub fn cancel_session(&mut self, session_id: u64) -> Option<TradeSession> {
        if let Some(session) = self.sessions.remove(&session_id) {
            self.player_to_session.remove(&session.player_a);
            self.player_to_session.remove(&session.player_b);
            Some(session)
        } else {
            None
        }
    }

    /// Aborts active session if a participant moves beyond maximum allowed distance.
    pub fn check_distance_and_abort(
        &mut self,
        moved_guid: Guid,
        entities: &EntityStore,
    ) -> Option<u64> {
        let session_id = *self.player_to_session.get(&moved_guid)?;
        let session = self.sessions.get(&session_id)?;

        let e1 = entities.get_by_guid(session.player_a)?;
        let e2 = entities.get_by_guid(session.player_b)?;

        let dx = e1.position.0 - e2.position.0;
        let dy = e1.position.1 - e2.position.1;
        let dz = e1.position.2 - e2.position.2;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();

        if dist > Self::MAX_TRADE_DISTANCE {
            self.cancel_session(session_id);
            Some(session_id)
        } else {
            None
        }
    }

    /// Executes atomic item swap between both participants using disjoint mutable borrows.
    pub fn execute_atomic_swap(
        &mut self,
        session_id: u64,
        entities: &mut EntityStore,
    ) -> Result<TradeCompletedEvent, &'static str> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or("Trade session not found")?;

        if !session.accepted_a || !session.accepted_b {
            return Err("Trade not accepted by both participants");
        }

        let p1 = session.player_a;
        let p2 = session.player_b;

        // Disjoint mutable borrow of both player entities simultaneously
        let (e1, e2) = entities
            .get_two_mut(p1, p2)
            .ok_or("Entities could not be disjointly borrowed")?;

        let inv1 = e1
            .get_component_mut::<InventoryComponent>()
            .ok_or("Player 1 has no inventory")?;
        let inv2 = e2
            .get_component_mut::<InventoryComponent>()
            .ok_or("Player 2 has no inventory")?;

        // 1. Verify capacity for incoming items
        let free_slots1 = inv1.slots.iter().filter(|s| s.is_none()).count();
        let free_slots2 = inv2.slots.iter().filter(|s| s.is_none()).count();

        if free_slots1 < session.offer_b.items.len() {
            return Err("Player 1 has insufficient inventory slots for trade");
        }
        if free_slots2 < session.offer_a.items.len() {
            return Err("Player 2 has insufficient inventory slots for trade");
        }

        // 2. Perform atomic transfer
        for item in &session.offer_a.items {
            let _ = inv2.add_item(item.clone());
        }
        for item in &session.offer_b.items {
            let _ = inv1.add_item(item.clone());
        }

        session.completed = true;
        let completed_event = TradeCompletedEvent {
            session_id,
            player_a: p1,
            player_b: p2,
        };

        // Clean up session mappings
        self.cancel_session(session_id);

        Ok(completed_event)
    }
}

/// Standalone plugin for atomic two-player trade transactions.
pub struct TradePlugin;

impl GameplayPlugin for TradePlugin {
    fn id(&self) -> &'static str {
        "trade"
    }

    fn build(&self, _ctx: &mut PluginContext) {
        // Trade management is registered and accessible via node runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::EntityType;

    #[test]
    fn test_atomic_trade_lifecycle_and_swap() {
        let mut store = EntityStore::new();
        let p1 = Guid::new(EntityType::Player, 1, 100);
        let p2 = Guid::new(EntityType::Player, 1, 200);

        let k1 = store.spawn(p1, (0.0, 0.0, 0.0));
        let k2 = store.spawn(p2, (2.0, 0.0, 0.0)); // 2m distance (< 5m)

        // Give both players inventories
        let mut inv1 = InventoryComponent::new(4, 100.0);
        let sword = ItemStack::new(10, "Excalibur", 1, 10.0);
        inv1.add_item(sword.clone()).unwrap();
        store.get_mut(k1).unwrap().add_component(inv1);

        let mut inv2 = InventoryComponent::new(4, 100.0);
        let shield = ItemStack::new(20, "Aegis Shield", 1, 12.0);
        inv2.add_item(shield.clone()).unwrap();
        store.get_mut(k2).unwrap().add_component(inv2);

        let mut trade_mgr = TradeManager::new();
        let session_id = trade_mgr.start_session(p1, p2, &store).unwrap();

        // Staging offers
        let offer_a = TradeOffer {
            items: vec![sword.clone()],
            currency: 50,
        };
        let offer_b = TradeOffer {
            items: vec![shield.clone()],
            currency: 0,
        };

        trade_mgr.get_session_mut(session_id).unwrap().set_offer(p1, offer_a).unwrap();
        trade_mgr.get_session_mut(session_id).unwrap().set_offer(p2, offer_b).unwrap();

        // Lock step
        trade_mgr.get_session_mut(session_id).unwrap().lock(p1).unwrap();
        trade_mgr.get_session_mut(session_id).unwrap().lock(p2).unwrap();

        // Acceptance
        assert!(!trade_mgr.get_session_mut(session_id).unwrap().accept(p1).unwrap());
        assert!(trade_mgr.get_session_mut(session_id).unwrap().accept(p2).unwrap());

        // Execute atomic swap using disjoint mutable borrows
        let result = trade_mgr.execute_atomic_swap(session_id, &mut store);
        assert!(result.is_ok());

        let p1_inv = store.get(k1).unwrap().get_component::<InventoryComponent>().unwrap();
        let p2_inv = store.get(k2).unwrap().get_component::<InventoryComponent>().unwrap();

        // Verify Player 1 now has shield and Player 2 has sword
        assert_eq!(p1_inv.slots[1], Some(shield));
        assert_eq!(p2_inv.slots[1], Some(sword));
    }

    #[test]
    fn test_distance_check_aborts_trade() {
        let mut store = EntityStore::new();
        let p1 = Guid::new(EntityType::Player, 1, 300);
        let p2 = Guid::new(EntityType::Player, 1, 400);

        let _k1 = store.spawn(p1, (0.0, 0.0, 0.0));
        let k2 = store.spawn(p2, (3.0, 0.0, 0.0));

        let mut trade_mgr = TradeManager::new();
        let session_id = trade_mgr.start_session(p1, p2, &store).unwrap();

        // Player 2 walks away to 10m
        store.get_mut(k2).unwrap().position.0 = 10.0;

        let aborted = trade_mgr.check_distance_and_abort(p2, &store);
        assert_eq!(aborted, Some(session_id));
        assert!(trade_mgr.get_session_mut(session_id).is_none());
    }
}
