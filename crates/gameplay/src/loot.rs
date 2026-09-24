//! Standalone Loot Tables and Container Gameplay Plugin.
//!
//! Provides probability-weighted drop tables, loot container components,
//! and player looting transfer mechanics into [`InventoryComponent`].

use serde::{Deserialize, Serialize};

use mmo_core::{GameplayPlugin, PluginContext};

use crate::inventory::{InventoryComponent, ItemStack};

/// Single entry in a loot table defining item identity, chance, and quantity range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LootEntry {
    pub item_id: u32,
    pub name: String,
    pub drop_chance: f32, // Probability in range [0.0, 1.0]
    pub min_count: u32,
    pub max_count: u32,
    pub unit_weight: f32,
}

impl LootEntry {
    pub fn new(
        item_id: u32,
        name: impl Into<String>,
        drop_chance: f32,
        min_count: u32,
        max_count: u32,
        unit_weight: f32,
    ) -> Self {
        Self {
            item_id,
            name: name.into(),
            drop_chance: drop_chance.clamp(0.0, 1.0),
            min_count,
            max_count: max_count.max(min_count),
            unit_weight,
        }
    }

    /// Evaluates if this entry drops given a normalized roll in [0.0, 1.0].
    pub fn roll(&self, roll_val: f32, count_val: f32) -> Option<ItemStack> {
        if roll_val <= self.drop_chance {
            let span = (self.max_count - self.min_count) as f32;
            let qty = self.min_count + (span * count_val).round() as u32;
            let final_qty = qty.clamp(self.min_count, self.max_count);
            Some(ItemStack::new(
                self.item_id,
                self.name.clone(),
                final_qty,
                self.unit_weight,
            ))
        } else {
            None
        }
    }
}

/// Collection of loot entries representing a monster or chest drop table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LootTable {
    pub table_id: u32,
    pub entries: Vec<LootEntry>,
}

impl LootTable {
    pub fn new(table_id: u32) -> Self {
        Self {
            table_id,
            entries: Vec::new(),
        }
    }

    pub fn add_entry(&mut self, entry: LootEntry) {
        self.entries.push(entry);
    }

    /// Generates loot item stacks using a deterministic pseudo-random seed.
    pub fn roll_loot(&self, seed: u64) -> Vec<ItemStack> {
        let mut drops = Vec::new();
        let mut state = seed;

        for entry in &self.entries {
            // Simple Linear Congruential Generator step
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let roll_val = ((state >> 32) as u32 as f32) / (u32::MAX as f32);

            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let count_val = ((state >> 32) as u32 as f32) / (u32::MAX as f32);

            if let Some(stack) = entry.roll(roll_val, count_val) {
                drops.push(stack);
            }
        }

        drops
    }
}

/// Dynamic component attached to corpses, chests, or world nodes holding loot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LootContainerComponent {
    pub items: Vec<ItemStack>,
}

impl LootContainerComponent {
    pub fn new(items: Vec<ItemStack>) -> Self {
        Self { items }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// Transfers as many items as possible from this container into the player's inventory.
    ///
    /// Returns the number of items successfully transferred.
    pub fn loot_all(&mut self, inventory: &mut InventoryComponent) -> usize {
        let mut unlooted = Vec::new();
        let mut transferred = 0;

        for item in self.items.drain(..) {
            if inventory.add_item(item.clone()).is_ok() {
                transferred += 1;
            } else {
                unlooted.push(item);
            }
        }

        self.items = unlooted;
        transferred
    }
}

/// Standalone plugin for loot tables and lootable containers.
pub struct LootPlugin;

impl GameplayPlugin for LootPlugin {
    fn id(&self) -> &'static str {
        "loot"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs
            .register::<LootContainerComponent>("loot_container");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loot_table_probability_rolls() {
        let mut table = LootTable::new(101);
        // Guaranteed drop (100%)
        table.add_entry(LootEntry::new(1, "Wolf Pelt", 1.0, 1, 2, 1.0));
        // Impossible drop (0%)
        table.add_entry(LootEntry::new(2, "Rare Gem", 0.0, 1, 1, 0.5));
        // Common drop (100% for test)
        table.add_entry(LootEntry::new(3, "Wolf Meat", 1.0, 2, 4, 0.5));

        let drops = table.roll_loot(42);
        assert_eq!(drops.len(), 2);
        assert_eq!(drops[0].item_id, 1);
        assert_eq!(drops[1].item_id, 3);
        assert!(drops[1].quantity >= 2 && drops[1].quantity <= 4);
    }

    #[test]
    fn test_loot_container_transfer_to_inventory() {
        let items = vec![
            ItemStack::new(10, "Gold Coin", 50, 0.01),
            ItemStack::new(11, "Iron Ingot", 5, 2.0),
            ItemStack::new(12, "Ruby", 1, 0.1),
        ];

        let mut container = LootContainerComponent::new(items);
        assert_eq!(container.count(), 3);

        // Player with capacity for 2 items only
        let mut player_inv = InventoryComponent::new(2, 100.0);
        let transferred = container.loot_all(&mut player_inv);

        assert_eq!(transferred, 2);
        assert_eq!(container.count(), 1); // 1 item remaining in corpse/chest
        assert_eq!(container.items[0].item_id, 12); // Ruby was left over
    }
}
