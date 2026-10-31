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
    #[serde(default)]
    pub currency: u64,
}

impl InventoryComponent {
    pub fn new(capacity: usize, max_weight: f32) -> Self {
        Self {
            slots: vec![None; capacity],
            max_weight,
            currency: 0,
        }
    }

    pub fn with_currency(mut self, currency: u64) -> Self {
        self.currency = currency;
        self
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

    pub fn has_item_stack(&self, item: &ItemStack) -> bool {
        if item.quantity == 0 {
            return false;
        }
        let total: u64 = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.item_id == item.item_id && s.name == item.name)
            .map(|s| s.quantity as u64)
            .sum();
        total >= item.quantity as u64
    }

    pub fn remove_item_stack(&mut self, item: &ItemStack) -> bool {
        if item.quantity == 0 || !self.has_item_stack(item) {
            return false;
        }

        let mut remaining = item.quantity;
        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                break;
            }
            if let Some(ref mut stack) = slot {
                if stack.item_id == item.item_id && stack.name == item.name {
                    if stack.quantity <= remaining {
                        remaining -= stack.quantity;
                        *slot = None;
                    } else {
                        stack.quantity -= remaining;
                        remaining = 0;
                    }
                }
            }
        }
        remaining == 0
    }

    pub fn add_item(&mut self, item: ItemStack) -> Result<usize, &'static str> {
        if item.quantity == 0 {
            return Err("Item quantity must be greater than zero");
        }
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
        // Fallback when entity store is not available
    }

    fn handle_with_entities(
        &self,
        event: &Event,
        ctx: &mut EventContext,
        entities: &mut EntityStore,
    ) {
        if let Event::BeforeMove { entity, .. } = event {
            if let Some(ent) = entities.get_by_guid(*entity) {
                if check_encumbrance(&ent.components) {
                    ctx.cancel("encumbered");
                }
            }
        }
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
                    4 => EquipSlot::Boots,
                    other => {
                        return Err(PacketHandlerError::MalformedPayload {
                            module_id: 0x01,
                            message: format!("Invalid equipment slot index {}", other),
                        });
                    }
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
                    // Put previous equipped item directly back into the vacated inventory slot
                    if let Some(inv) = entity.get_component_mut::<InventoryComponent>() {
                        if inv_slot < inv.slots.len() && inv.slots[inv_slot].is_none() {
                            inv.slots[inv_slot] = Some(old);
                        } else {
                            let _ = inv.add_item(old);
                        }
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
        ctx.hooks.register_handler(Box::new(EncumbranceHookHandler));
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

    #[test]
    fn test_inventory_remove_and_bounds() {
        let mut inv = InventoryComponent::new(2, 100.0);
        let item = ItemStack::new(1, "Dagger", 1, 2.0);
        let slot = inv.add_item(item.clone()).unwrap();
        assert_eq!(slot, 0);

        assert_eq!(inv.remove_item(10), None);
        assert_eq!(inv.remove_item(0), Some(item));
        assert_eq!(inv.remove_item(0), None);
    }

    #[test]
    fn test_check_encumbrance_helper() {
        let mut store = ComponentStore::new();
        assert!(!check_encumbrance(&store));

        let mut inv = InventoryComponent::new(2, 10.0);
        inv.add_item(ItemStack::new(1, "Anvil", 1, 15.0)).unwrap();
        store.add_component(inv);
        assert!(check_encumbrance(&store));
    }

    #[test]
    fn test_inventory_plugin_and_hook() {
        use mmo_core::{
            ComponentCodecRegistry, HookDispatcher, ModularPacketRouter, PluginManager,
        };

        let plugin = InventoryPlugin;
        assert_eq!(plugin.id(), "inventory");

        let mut mgr = PluginManager::new();
        mgr.add_plugin(Box::new(InventoryPlugin));
        let mut hooks = HookDispatcher::new();
        let mut codecs = ComponentCodecRegistry::new();
        let mut event_queue = mmo_core::EventQueue::new();
        let mut packets = ModularPacketRouter::new();
        let mut tick_systems = Vec::new();
        mgr.build_all(
            &mut hooks,
            &mut codecs,
            &mut event_queue,
            &mut packets,
            &mut tick_systems,
        );

        let handler = EncumbranceHookHandler;
        let mut event_ctx = EventContext::new();
        let event = Event::BeforeMove {
            entity: Guid::new(mmo_core::EntityType::Player, 1, 1),
            from: (0.0, 0.0, 0.0),
            to: (1.0, 0.0, 0.0),
        };
        handler.handle(&event, &mut event_ctx);
    }

    #[test]
    fn test_inventory_packet_handler_flows() {
        use mmo_core::EntityType;

        let handler = InventoryPacketHandler;
        let mut entities = EntityStore::new();
        let guid = Guid::new(EntityType::Player, 1, 42);

        // 1. Entity not found
        let err = handler.handle(guid, 1, &[0, 0], &mut entities);
        assert!(matches!(err, Err(PacketHandlerError::EntityNotFound(_))));

        // Create entity without inventory
        entities.spawn(guid, (0.0, 0.0, 0.0));

        // 2. Malformed payload
        let err = handler.handle(guid, 1, &[0], &mut entities);
        assert!(matches!(
            err,
            Err(PacketHandlerError::MalformedPayload { .. })
        ));

        // 3. Unknown sub-opcode
        let err = handler.handle(guid, 99, &[0, 0], &mut entities);
        assert!(matches!(
            err,
            Err(PacketHandlerError::MalformedPayload { .. })
        ));

        // 4. Missing InventoryComponent
        let err = handler.handle(guid, 1, &[0, 0], &mut entities);
        assert!(matches!(err, Err(PacketHandlerError::Custom(_))));

        // Add inventory
        let inv = InventoryComponent::new(5, 100.0);
        let entity = entities.get_mut_by_guid(guid).unwrap();
        entity.add_component(inv);

        // 5. Empty slot
        let err = handler.handle(guid, 1, &[0, 0], &mut entities);
        assert!(matches!(err, Err(PacketHandlerError::Custom(_))));

        // 6. Successful equip (Head = 0)
        let entity = entities.get_mut_by_guid(guid).unwrap();
        let inv_mut = entity.get_component_mut::<InventoryComponent>().unwrap();
        inv_mut
            .add_item(ItemStack::new(50, "Helm", 1, 5.0))
            .unwrap();
        inv_mut
            .add_item(ItemStack::new(51, "Crown", 1, 4.0))
            .unwrap();

        let ok = handler.handle(guid, 1, &[0, 0], &mut entities);
        assert_eq!(ok.unwrap(), Some(vec![1]));

        // 7. Equip second item into same slot (swap / returns old to inventory)
        let ok = handler.handle(guid, 1, &[1, 0], &mut entities);
        assert_eq!(ok.unwrap(), Some(vec![1]));

        // Verify equipment has Crown and inventory received old Helm
        let entity = entities.get_mut_by_guid(guid).unwrap();
        let eq = entity.get_component::<EquipmentComponent>().unwrap();
        assert_eq!(eq.get_equipped(&EquipSlot::Head).unwrap().item_id, 51);
        let inv = entity.get_component::<InventoryComponent>().unwrap();
        assert_eq!(inv.slots[1].as_ref().unwrap().item_id, 50);

        // 8. Equip into Chest (1), MainHand (2), OffHand (3), Boots (4)
        for (sub_slot, equip_slot) in [
            (1, EquipSlot::Chest),
            (2, EquipSlot::MainHand),
            (3, EquipSlot::OffHand),
            (4, EquipSlot::Boots),
        ] {
            let entity = entities.get_mut_by_guid(guid).unwrap();
            let inv_mut = entity.get_component_mut::<InventoryComponent>().unwrap();
            let idx = inv_mut
                .add_item(ItemStack::new(100 + sub_slot as u32, "Gear", 1, 1.0))
                .unwrap();
            let ok = handler.handle(guid, 1, &[idx as u8, sub_slot], &mut entities);
            assert!(ok.is_ok());
            let entity = entities.get_mut_by_guid(guid).unwrap();
            let eq = entity.get_component::<EquipmentComponent>().unwrap();
            assert!(eq.get_equipped(&equip_slot).is_some());
        }

        // 9. Invalid slot index (> 4) returns MalformedPayload
        let entity = entities.get_mut_by_guid(guid).unwrap();
        let inv_mut = entity.get_component_mut::<InventoryComponent>().unwrap();
        let idx = inv_mut
            .add_item(ItemStack::new(999, "Invalid", 1, 1.0))
            .unwrap();
        let err = handler.handle(guid, 1, &[idx as u8, 5], &mut entities);
        assert!(matches!(
            err,
            Err(PacketHandlerError::MalformedPayload { .. })
        ));
    }

    #[test]
    fn test_encumbrance_hook_handler_cancels_before_move() {
        use mmo_core::EntityType;

        let mut store = EntityStore::new();
        let player = Guid::new(EntityType::Player, 1, 777);
        let key = store.spawn(player, (0.0, 0.0, 0.0));

        // Capacity 2, max_weight 10.0
        let mut inv = InventoryComponent::new(2, 10.0);
        let heavy = ItemStack::new(1, "Boulders", 1, 25.0); // 25.0 > 10.0 -> encumbered
        inv.add_item(heavy).unwrap();
        store.get_mut(key).unwrap().add_component(inv);

        let handler = EncumbranceHookHandler;
        let ev = Event::BeforeMove {
            entity: player,
            from: (0.0, 0.0, 0.0),
            to: (1.0, 0.0, 0.0),
        };
        let mut ctx = EventContext::new();

        handler.handle_with_entities(&ev, &mut ctx, &mut store);
        assert!(ctx.is_canceled());
        assert_eq!(ctx.cancel_reason(), Some("encumbered"));
    }

    #[test]
    fn test_zero_quantity_and_multi_stack_overflow_guards() {
        let mut inv = InventoryComponent::new(4, 100.0);

        // 1. Cannot add item with quantity == 0
        let zero_item = ItemStack::new(100, "Ghost", 0, 1.0);
        assert_eq!(
            inv.add_item(zero_item.clone()),
            Err("Item quantity must be greater than zero")
        );

        // 2. has_item_stack returns false for quantity == 0 even if item is not present
        assert!(!inv.has_item_stack(&zero_item));

        // Add 2 stacks of 2,000,000,000 quantity
        let big_stack_1 = ItemStack::new(101, "Gold Bars", 2_000_000_000, 0.001);
        let big_stack_2 = ItemStack::new(101, "Gold Bars", 2_000_000_000, 0.001);
        inv.add_item(big_stack_1).unwrap();
        inv.add_item(big_stack_2).unwrap();

        // 3. has_item_stack returns false for quantity == 0 even if item IS present
        let zero_gold = ItemStack::new(101, "Gold Bars", 0, 0.001);
        assert!(!inv.has_item_stack(&zero_gold));
        assert!(!inv.remove_item_stack(&zero_gold));

        // 4. Multi-stack sum exceeds u32::MAX (4,000,000,000 > 3,000,000,000)
        let query = ItemStack::new(101, "Gold Bars", 3_000_000_000, 0.001);
        assert!(inv.has_item_stack(&query));

        // 5. Remove spanning across stacks
        assert!(inv.remove_item_stack(&query));
        assert_eq!(inv.slots[0], None);
        assert_eq!(inv.slots[1].as_ref().unwrap().quantity, 1_000_000_000);
    }
}
