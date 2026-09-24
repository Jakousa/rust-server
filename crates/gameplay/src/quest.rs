//! Standalone Quest System Gameplay Plugin.
//!
//! Provides structured quest definitions, objective progression tracking,
//! event-driven objective completion, and quest reward distribution.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use mmo_core::{GameplayPlugin, PluginContext};

use crate::inventory::{InventoryComponent, ItemStack};

/// Types of objectives required to complete a quest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuestObjective {
    /// Slaying monsters of a specific type.
    KillMonster {
        monster_type_id: u32,
        required: u32,
        current: u32,
    },
    /// Gathering item stacks into inventory.
    FetchItem {
        item_id: u32,
        required: u32,
        current: u32,
    },
}

impl QuestObjective {
    pub fn is_complete(&self) -> bool {
        match *self {
            Self::KillMonster { required, current, .. } => current >= required,
            Self::FetchItem { required, current, .. } => current >= required,
        }
    }

    pub fn advance_kill(&mut self, target_type: u32) -> bool {
        if let Self::KillMonster { monster_type_id, required, current } = self {
            if *monster_type_id == target_type && *current < *required {
                *current += 1;
                return true;
            }
        }
        false
    }

    pub fn advance_fetch(&mut self, target_item: u32, count: u32) -> bool {
        if let Self::FetchItem { item_id, required, current } = self {
            if *item_id == target_item && *current < *required {
                *current = (*current + count).min(*required);
                return true;
            }
        }
        false
    }
}

/// Lifecycle status of an active or finished quest.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestStatus {
    InProgress,
    ReadyForTurnIn,
    Completed,
}

/// Active tracking record for a single quest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestRecord {
    pub quest_id: u32,
    pub name: String,
    pub objectives: Vec<QuestObjective>,
    pub status: QuestStatus,
    pub reward_items: Vec<ItemStack>,
    pub reward_xp: u32,
}

impl QuestRecord {
    pub fn new(
        quest_id: u32,
        name: impl Into<String>,
        objectives: Vec<QuestObjective>,
        reward_items: Vec<ItemStack>,
        reward_xp: u32,
    ) -> Self {
        Self {
            quest_id,
            name: name.into(),
            objectives,
            status: QuestStatus::InProgress,
            reward_items,
            reward_xp,
        }
    }

    /// Evaluates all objectives and updates status to `ReadyForTurnIn` if all are complete.
    pub fn update_status(&mut self) -> QuestStatus {
        if self.status == QuestStatus::InProgress {
            let all_done = self.objectives.iter().all(|obj| obj.is_complete());
            if all_done {
                self.status = QuestStatus::ReadyForTurnIn;
            }
        }
        self.status
    }
}

/// Dynamic component attached to player entities storing their quest progression.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QuestLogComponent {
    pub active_quests: HashMap<u32, QuestRecord>,
    pub completed_quests: Vec<u32>,
}

impl QuestLogComponent {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts and adds a new quest into the log.
    pub fn accept_quest(&mut self, quest: QuestRecord) -> Result<(), &'static str> {
        if self.completed_quests.contains(&quest.quest_id) {
            return Err("Quest already completed");
        }
        if self.active_quests.contains_key(&quest.quest_id) {
            return Err("Quest already in progress");
        }
        self.active_quests.insert(quest.quest_id, quest);
        Ok(())
    }

    /// Advances kill objectives across all active quests when a monster dies.
    pub fn on_monster_killed(&mut self, monster_type: u32) -> Vec<u32> {
        let mut newly_ready = Vec::new();

        for (quest_id, quest) in self.active_quests.iter_mut() {
            let mut advanced = false;
            for obj in &mut quest.objectives {
                if obj.advance_kill(monster_type) {
                    advanced = true;
                }
            }
            if advanced && quest.update_status() == QuestStatus::ReadyForTurnIn {
                newly_ready.push(*quest_id);
            }
        }

        newly_ready
    }

    /// Turns in a completed quest, depositing rewards into player inventory.
    ///
    /// Returns the rewarded experience points upon success.
    pub fn turn_in_quest(
        &mut self,
        quest_id: u32,
        inventory: &mut InventoryComponent,
    ) -> Result<u32, &'static str> {
        let quest = self
            .active_quests
            .get(&quest_id)
            .ok_or("Quest not found in active log")?;

        if quest.status != QuestStatus::ReadyForTurnIn {
            return Err("Quest is not ready for turn-in");
        }

        // Verify inventory capacity for reward items
        let free_slots = inventory.slots.iter().filter(|s| s.is_none()).count();
        if free_slots < quest.reward_items.len() {
            return Err("Not enough inventory space for quest rewards");
        }

        let mut completed_quest = self.active_quests.remove(&quest_id).unwrap();
        completed_quest.status = QuestStatus::Completed;

        // Distribute items
        for reward in completed_quest.reward_items {
            let _ = inventory.add_item(reward);
        }

        self.completed_quests.push(quest_id);
        Ok(completed_quest.reward_xp)
    }
}

/// Standalone plugin for quest progression and rewards.
pub struct QuestPlugin;

impl GameplayPlugin for QuestPlugin {
    fn id(&self) -> &'static str {
        "quest"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<QuestLogComponent>("quest_log");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quest_progression_and_reward_turn_in() {
        let mut log = QuestLogComponent::new();

        let quest = QuestRecord::new(
            1,
            "Wolf Menace",
            vec![QuestObjective::KillMonster {
                monster_type_id: 2, // Npc/Wolf
                required: 2,
                current: 0,
            }],
            vec![ItemStack::new(50, "Wolf Slayer Dagger", 1, 3.0)],
            250, // XP
        );

        assert!(log.accept_quest(quest).is_ok());

        // First kill: not ready yet
        let ready = log.on_monster_killed(2);
        assert!(ready.is_empty());
        assert_eq!(
            log.active_quests.get(&1).unwrap().status,
            QuestStatus::InProgress
        );

        // Second kill: quest becomes ready!
        let ready = log.on_monster_killed(2);
        assert_eq!(ready, vec![1]);
        assert_eq!(
            log.active_quests.get(&1).unwrap().status,
            QuestStatus::ReadyForTurnIn
        );

        // Turn in quest to receive dagger and XP
        let mut inv = InventoryComponent::new(4, 50.0);
        let xp = log.turn_in_quest(1, &mut inv).unwrap();
        assert_eq!(xp, 250);
        assert_eq!(inv.slots[0].as_ref().unwrap().item_id, 50);
        assert!(log.completed_quests.contains(&1));
    }
}
