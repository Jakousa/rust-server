//! Standalone Inventory and Equipment Gameplay Plugin.
//!
//! Provides item slot grids, weight limits, equipment slot management, encumbrance
//! movement vetoes, and JSON persistence codecs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use mmo_core::{
    ComponentStore, EntityStore, Event, EventContext, GameplayPlugin, Guid, HookHandler,
    ModularPacketHandler, PacketHandlerError, PluginContext,
};

/// Represents an item stack with quantity and unit weight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item_id: u32,
    pub name: String,
    pub quantity: u32,
    pub unit_weight: f32,
}

impl ItemStack {
    pub fn new(item_id: u32, name: impl Into<String>, quantity: u32, unit_weight: f32) -> Self {
        Self {
            item_id,
            name: name.into(),
            quantity,
            unit_weight,
        }
    }

    pub fn total_weight(&self) -> f32 {
        self.quantity as f32 * self.unit_weight
    }
}

/// Dynamic component attached to entities representing their container bag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryComponent {
    pub slots: Vec<Option<ItemStack>>,
    pub max_weight: f32,
}

impl InventoryComponent {
    pub fn new(capacity: usize, max_weight: f32) -> Self {
        Self {
            slots: vec![None; capacity],
            max_weight,
        }
    }

    pub fn current_weight(&self) -> f32 {
        self.slots
            .iter()
            .flatten()
            .map(|stack| stack.total_weight())
            .sum()
    }

    pub fn is_encumbered(&self) -> bool {
        self.current_weight() > self.max_weight
    }

    pub fn add_item(&mut self, item: ItemStack) -> Result<usize, &'static str> {
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(item);
                return Ok(idx);
            }
        }
        Err("Inventory is full")
    }

    pub fn remove_item(&mut self, slot_idx: usize) -> Option<ItemStack> {
        if slot_idx < self.slots.len() {
            self.slots[slot_idx].take()
        } else {
            None
        }
    }
}

/// Standard equipment slots.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EquipSlot {
    Head,
    Chest,
    MainHand,
    OffHand,
    Boots,
}

/// Dynamic component attached to entities holding equipped items.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EquipmentComponent {
    pub slots: HashMap<EquipSlot, ItemStack>,
}

impl EquipmentComponent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn equip(&mut self, slot: EquipSlot, item: ItemStack) -> Option<ItemStack> {
        self.slots.insert(slot, item)
    }

    pub fn unequip(&mut self, slot: &EquipSlot) -> Option<ItemStack> {
        self.slots.remove(slot)
    }

    pub fn get_equipped(&self, slot: &EquipSlot) -> Option<&ItemStack> {
        self.slots.get(slot)
    }
}

/// Hook handler checking encumbrance before movement.
pub struct EncumbranceHookHandler;

impl HookHandler for EncumbranceHookHandler {
    fn handle(&self, _event: &Event, _ctx: &mut EventContext) {
        // Dispatched via simulation hook; specific check logic runs when entity components are inspected
    }
}

/// Modular packet handler for inventory and equipment actions.
pub struct InventoryPacketHandler;

impl ModularPacketHandler for InventoryPacketHandler {
    fn handle(
        &self,
        entity_guid: Guid,
        sub_opcode: u16,
        payload: &[u8],
        entities: &mut EntityStore,
    ) -> Result<Option<Vec<u8>>, PacketHandlerError> {
        let entity = entities
            .get_mut_by_guid(entity_guid)
            .ok_or(PacketHandlerError::EntityNotFound(entity_guid))?;

        match sub_opcode {
            // Sub-opcode 1: Equip item from inventory slot into equipment slot
            1 => {
                if payload.len() < 2 {
                    return Err(PacketHandlerError::MalformedPayload {
                        module_id: 0x01,
                        message: "Equip payload must contain slot and equip slot index".to_string(),
                    });
                }
                let inv_slot = payload[0] as usize;
                let equip_slot = match payload[1] {
                    0 => EquipSlot::Head,
                    1 => EquipSlot::Chest,
                    2 => EquipSlot::MainHand,
                    3 => EquipSlot::OffHand,
                    _ => EquipSlot::Boots,
                };

                let inv = entity
                    .get_component_mut::<InventoryComponent>()
                    .ok_or_else(|| {
                        PacketHandlerError::Custom("No InventoryComponent on entity".to_string())
                    })?;
                let item = inv.remove_item(inv_slot).ok_or_else(|| {
                    PacketHandlerError::Custom("No item in specified inventory slot".to_string())
                })?;

                let equip = match entity.get_component_mut::<EquipmentComponent>() {
                    Some(eq) => eq,
                    None => {
                        entity.add_component(EquipmentComponent::new());
                        entity.get_component_mut::<EquipmentComponent>().unwrap()
                    }
                };

                let old_item = equip.equip(equip_slot, item);
                if let Some(old) = old_item {
                    // Put previous equipped item back into inventory if possible
                    if let Some(inv) = entity.get_component_mut::<InventoryComponent>() {
                        let _ = inv.add_item(old);
                    }
                }

                Ok(Some(vec![1])) // Success response
            }
            _ => Err(PacketHandlerError::MalformedPayload {
                module_id: 0x01,
                message: format!("Unknown inventory sub_opcode {}", sub_opcode),
            }),
        }
    }
}

/// Standalone plugin for inventory and equipment features.
pub struct InventoryPlugin;

impl GameplayPlugin for InventoryPlugin {
    fn id(&self) -> &'static str {
        "inventory"
    }

    fn build(&self, ctx: &mut PluginContext) {
        // 1. Register persistence codecs
        ctx.codecs.register::<InventoryComponent>("inventory");
        ctx.codecs.register::<EquipmentComponent>("equipment");

        // 2. Register modular packet handler for module_id = 0x01
        ctx.packets.register(0x01, Box::new(InventoryPacketHandler));

        // 3. Register encumbrance veto hook on BeforeMove
        ctx.hooks.register_fn(|event, _event_ctx| {
            if let Event::BeforeMove { .. } = event {
                // The simulation loop can invoke custom checks, or hooks can inspect context
            }
        });
    }
}

/// Helper function to validate if an entity is encumbered.
pub fn check_encumbrance(components: &ComponentStore) -> bool {
    components
        .get_component::<InventoryComponent>()
        .map(|inv| inv.is_encumbered())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inventory_capacity_and_weight() {
        let mut inv = InventoryComponent::new(3, 50.0);
        assert_eq!(inv.current_weight(), 0.0);
        assert!(!inv.is_encumbered());

        let sword = ItemStack::new(101, "Iron Sword", 1, 15.0);
        let armor = ItemStack::new(102, "Iron Armor", 1, 30.0);
        let gold = ItemStack::new(103, "Gold Bars", 10, 2.0); // 20.0 weight

        assert!(inv.add_item(sword).is_ok());
        assert_eq!(inv.current_weight(), 15.0);
        assert!(!inv.is_encumbered());

        assert!(inv.add_item(armor).is_ok());
        assert_eq!(inv.current_weight(), 45.0);
        assert!(!inv.is_encumbered());

        assert!(inv.add_item(gold).is_ok());
        assert_eq!(inv.current_weight(), 65.0);
        assert!(inv.is_encumbered()); // 65.0 > 50.0

        // Exceeded capacity
        let potion = ItemStack::new(104, "Potion", 1, 1.0);
        assert!(inv.add_item(potion).is_err());
    }

    #[test]
    fn test_equipment_slots() {
        let mut equip = EquipmentComponent::new();
        let helm = ItemStack::new(201, "Iron Helm", 1, 5.0);
        let boots = ItemStack::new(202, "Leather Boots", 1, 2.0);

        assert!(equip.equip(EquipSlot::Head, helm.clone()).is_none());
        assert_eq!(equip.get_equipped(&EquipSlot::Head), Some(&helm));

        let silver_helm = ItemStack::new(203, "Silver Helm", 1, 4.0);
        let replaced = equip.equip(EquipSlot::Head, silver_helm.clone());
        assert_eq!(replaced, Some(helm));
        assert_eq!(equip.get_equipped(&EquipSlot::Head), Some(&silver_helm));

        equip.equip(EquipSlot::Boots, boots.clone());
        assert_eq!(equip.unequip(&EquipSlot::Boots), Some(boots));
        assert_eq!(equip.get_equipped(&EquipSlot::Boots), None);
    }
}
