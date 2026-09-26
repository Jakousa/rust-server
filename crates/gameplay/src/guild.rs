//! Standalone Guilds Management Gameplay Plugin.
//!
//! Provides guild rosters, permission hierarchies (Member, Officer, Leader),
//! and a shared transactional [`GuildVault`] bank container.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use mmo_core::{Entity, GameplayPlugin, Guid, PluginContext};

use crate::inventory::{InventoryComponent, ItemStack};

/// Ranks within a guild hierarchy defining administrative permissions.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GuildRank {
    Member = 1,
    Officer = 2,
    Leader = 3,
}

/// Dynamic component attached to player entities representing their guild affiliation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuildMemberComponent {
    pub guild_id: u64,
    pub rank: GuildRank,
}

impl GuildMemberComponent {
    pub fn new(guild_id: u64, rank: GuildRank) -> Self {
        Self { guild_id, rank }
    }
}

/// Shared bank container for storing pooled guild items.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GuildVault {
    pub slots: Vec<Option<ItemStack>>,
}

impl GuildVault {
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: vec![None; capacity],
        }
    }

    pub fn deposit(&mut self, item: ItemStack) -> Result<usize, &'static str> {
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(item);
                return Ok(idx);
            }
        }
        Err("Guild vault is full")
    }

    pub fn withdraw(&mut self, slot_idx: usize) -> Option<ItemStack> {
        if slot_idx < self.slots.len() {
            self.slots[slot_idx].take()
        } else {
            None
        }
    }
}

/// In-memory state of a guild organization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuildState {
    pub guild_id: u64,
    pub name: String,
    pub leader: Guid,
    pub members: HashMap<Guid, GuildRank>,
    pub vault: GuildVault,
}

impl GuildState {
    pub fn new(
        guild_id: u64,
        name: impl Into<String>,
        leader: Guid,
        vault_capacity: usize,
    ) -> Self {
        let mut members = HashMap::new();
        members.insert(leader, GuildRank::Leader);

        Self {
            guild_id,
            name: name.into(),
            leader,
            members,
            vault: GuildVault::new(vault_capacity),
        }
    }

    pub fn add_member(&mut self, member: Guid, rank: GuildRank) {
        self.members.insert(member, rank);
    }

    pub fn remove_member(&mut self, member: Guid) -> Option<GuildRank> {
        if member == self.leader {
            None // Leader cannot simply be removed without transferring leadership
        } else {
            self.members.remove(&member)
        }
    }

    pub fn get_rank(&self, member: Guid) -> Option<GuildRank> {
        self.members.get(&member).copied()
    }
}

/// Manager coordinating guild creation, roster lookups, and vault transactions.
#[derive(Default)]
pub struct GuildManager {
    guilds: HashMap<u64, GuildState>,
    player_to_guild: HashMap<Guid, u64>,
    next_guild_id: u64,
}

impl GuildManager {
    pub fn new() -> Self {
        Self {
            guilds: HashMap::new(),
            player_to_guild: HashMap::new(),
            next_guild_id: 1,
        }
    }

    /// Creates a new guild with the specified founding leader and vault size.
    pub fn create_guild(&mut self, name: &str, leader: Guid, vault_capacity: usize) -> u64 {
        let guild_id = self.next_guild_id;
        self.next_guild_id += 1;

        let guild = GuildState::new(guild_id, name, leader, vault_capacity);
        self.guilds.insert(guild_id, guild);
        self.player_to_guild.insert(leader, guild_id);

        guild_id
    }

    pub fn get_guild(&self, guild_id: u64) -> Option<&GuildState> {
        self.guilds.get(&guild_id)
    }

    pub fn get_guild_mut(&mut self, guild_id: u64) -> Option<&mut GuildState> {
        self.guilds.get_mut(&guild_id)
    }

    /// Adds a member to an existing guild, updating roster and mapping.
    pub fn add_member(
        &mut self,
        guild_id: u64,
        player: Guid,
        rank: GuildRank,
    ) -> Result<(), &'static str> {
        let guild = self.guilds.get_mut(&guild_id).ok_or("Guild not found")?;
        guild.add_member(player, rank);
        self.player_to_guild.insert(player, guild_id);
        Ok(())
    }

    /// Atomically transfers an item from player's inventory to the shared guild vault.
    pub fn deposit_item(
        &mut self,
        player_guid: Guid,
        inv_slot: usize,
        player_entity: &mut Entity,
    ) -> Result<(), &'static str> {
        let guild_id = *self
            .player_to_guild
            .get(&player_guid)
            .ok_or("Player is not in a guild")?;
        let guild = self.guilds.get_mut(&guild_id).ok_or("Guild not found")?;

        let inv = player_entity
            .get_component_mut::<InventoryComponent>()
            .ok_or("Player has no inventory component")?;

        let item = inv
            .remove_item(inv_slot)
            .ok_or("No item in specified slot")?;

        if let Err(err) = guild.vault.deposit(item.clone()) {
            // Rollback item back to player inventory
            let _ = inv.add_item(item);
            return Err(err);
        }

        Ok(())
    }

    /// Atomically transfers an item from the shared guild vault to player's inventory.
    ///
    /// Requires Officer or Leader rank.
    pub fn withdraw_item(
        &mut self,
        player_guid: Guid,
        vault_slot: usize,
        player_entity: &mut Entity,
    ) -> Result<(), &'static str> {
        let guild_id = *self
            .player_to_guild
            .get(&player_guid)
            .ok_or("Player is not in a guild")?;
        let guild = self.guilds.get_mut(&guild_id).ok_or("Guild not found")?;

        let rank = guild
            .get_rank(player_guid)
            .ok_or("Player not found in guild roster")?;
        if rank < GuildRank::Officer {
            return Err("Must be an Officer or Leader to withdraw from the guild vault");
        }

        let inv = player_entity
            .get_component_mut::<InventoryComponent>()
            .ok_or("Player has no inventory component")?;

        let item = guild
            .vault
            .withdraw(vault_slot)
            .ok_or("No item in specified vault slot")?;

        if let Err(err) = inv.add_item(item.clone()) {
            // Rollback item back into vault
            let _ = guild.vault.deposit(item);
            return Err(err);
        }

        Ok(())
    }
}

/// Standalone plugin for guild organization and shared vault management.
pub struct GuildPlugin;

impl GameplayPlugin for GuildPlugin {
    fn id(&self) -> &'static str {
        "guild"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<GuildMemberComponent>("guild_member");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    #[test]
    fn test_guild_creation_and_vault_transactions() {
        let leader_guid = Guid::new(EntityType::Player, 1, 1);
        let officer_guid = Guid::new(EntityType::Player, 1, 2);
        let member_guid = Guid::new(EntityType::Player, 1, 3);

        let mut mgr = GuildManager::new();
        let guild_id = mgr.create_guild("Knights of the Octree", leader_guid, 5);

        // Add officer and member
        let guild = mgr.get_guild_mut(guild_id).unwrap();
        guild.add_member(officer_guid, GuildRank::Officer);
        guild.add_member(member_guid, GuildRank::Member);
        mgr.player_to_guild.insert(officer_guid, guild_id);
        mgr.player_to_guild.insert(member_guid, guild_id);

        // Member deposits an item into the vault
        let mut member_entity = Entity::new(member_guid, (0.0, 0.0, 0.0));
        let mut inv = InventoryComponent::new(4, 100.0);
        let ore = ItemStack::new(500, "Dark Iron Ore", 20, 1.0);
        inv.add_item(ore.clone()).unwrap();
        member_entity.add_component(inv);

        assert!(mgr.deposit_item(member_guid, 0, &mut member_entity).is_ok());
        assert_eq!(
            mgr.get_guild(guild_id).unwrap().vault.slots[0],
            Some(ore.clone())
        );

        // Normal member tries to withdraw -> Permission denied!
        assert!(mgr
            .withdraw_item(member_guid, 0, &mut member_entity)
            .is_err());

        // Officer withdraws -> Success!
        let mut officer_entity = Entity::new(officer_guid, (0.0, 0.0, 0.0));
        officer_entity.add_component(InventoryComponent::new(4, 100.0));

        assert!(mgr
            .withdraw_item(officer_guid, 0, &mut officer_entity)
            .is_ok());
        let officer_inv = officer_entity
            .get_component::<InventoryComponent>()
            .unwrap();
        assert_eq!(officer_inv.slots[0], Some(ore));
        assert_eq!(mgr.get_guild(guild_id).unwrap().vault.slots[0], None);
    }
}
