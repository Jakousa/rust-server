//! Standalone Party System Gameplay Plugin.
//!
//! Provides group creation, invitation staging, member roster synchronization,
//! and leadership delegation across player entities.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use mmo_core::{GameplayPlugin, Guid, PluginContext};

/// Dynamic component attached to player entities representing active party membership.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartyMemberComponent {
    pub party_id: u64,
    pub is_leader: bool,
}

impl PartyMemberComponent {
    pub fn new(party_id: u64, is_leader: bool) -> Self {
        Self {
            party_id,
            is_leader,
        }
    }
}

/// State representation of an active adventuring party.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartyState {
    pub party_id: u64,
    pub leader: Guid,
    pub members: Vec<Guid>,
    pub max_members: usize,
}

impl PartyState {
    pub const DEFAULT_MAX_MEMBERS: usize = 5;

    pub fn new(party_id: u64, leader: Guid) -> Self {
        Self {
            party_id,
            leader,
            members: vec![leader],
            max_members: Self::DEFAULT_MAX_MEMBERS,
        }
    }

    pub fn add_member(&mut self, member: Guid) -> Result<(), &'static str> {
        if self.members.contains(&member) {
            return Err("Player already in party");
        }
        if self.members.len() >= self.max_members {
            return Err("Party is full");
        }
        self.members.push(member);
        Ok(())
    }

    /// Removes a member from the party.
    ///
    /// If the departing member is the leader, passes leadership to the next remaining member.
    /// Returns `true` if the party has no remaining members and should be disbanded.
    pub fn remove_member(&mut self, member: Guid) -> Result<bool, &'static str> {
        let index = self
            .members
            .iter()
            .position(|&g| g == member)
            .ok_or("Player not found in party")?;

        self.members.remove(index);

        if self.members.is_empty() {
            return Ok(true); // Disbanded
        }

        if self.leader == member {
            self.leader = self.members[0];
        }

        Ok(false)
    }

    pub fn set_leader(&mut self, new_leader: Guid) -> Result<(), &'static str> {
        if !self.members.contains(&new_leader) {
            return Err("New leader must be an existing party member");
        }
        self.leader = new_leader;
        Ok(())
    }
}

/// Coordinates party formation, pending invitations, and roster lookups.
#[derive(Default)]
pub struct PartyManager {
    parties: HashMap<u64, PartyState>,
    player_to_party: HashMap<Guid, u64>,
    pending_invites: HashMap<Guid, (u64, Guid)>, // invitee -> (party_id, inviter)
    next_party_id: u64,
}

impl PartyManager {
    pub fn new() -> Self {
        Self {
            parties: HashMap::new(),
            player_to_party: HashMap::new(),
            pending_invites: HashMap::new(),
            next_party_id: 1,
        }
    }

    /// Creates a new party with the given founding leader.
    pub fn create_party(&mut self, leader: Guid) -> Result<u64, &'static str> {
        if self.player_to_party.contains_key(&leader) {
            return Err("Player already in a party");
        }

        let party_id = self.next_party_id;
        self.next_party_id += 1;

        let party = PartyState::new(party_id, leader);
        self.parties.insert(party_id, party);
        self.player_to_party.insert(leader, party_id);

        Ok(party_id)
    }

    pub fn get_party(&self, party_id: u64) -> Option<&PartyState> {
        self.parties.get(&party_id)
    }

    pub fn get_player_party(&self, player: Guid) -> Option<&PartyState> {
        let party_id = self.player_to_party.get(&player)?;
        self.parties.get(party_id)
    }

    /// Staging an invite from an existing party leader to an invitee.
    pub fn invite_player(&mut self, inviter: Guid, invitee: Guid) -> Result<u64, &'static str> {
        let party_id = *self
            .player_to_party
            .get(&inviter)
            .ok_or("Inviter is not in a party")?;

        let party = self.parties.get(&party_id).ok_or("Party not found")?;
        if party.leader != inviter {
            return Err("Only party leader can invite players");
        }

        if self.player_to_party.contains_key(&invitee) {
            return Err("Invitee is already in a party");
        }

        self.pending_invites.insert(invitee, (party_id, inviter));
        Ok(party_id)
    }

    /// Invitee accepts the pending invitation and joins the party.
    pub fn accept_invite(&mut self, invitee: Guid) -> Result<u64, &'static str> {
        let (party_id, _) = self
            .pending_invites
            .remove(&invitee)
            .ok_or("No pending party invitation for player")?;

        let party = self.parties.get_mut(&party_id).ok_or("Party no longer exists")?;
        party.add_member(invitee)?;
        self.player_to_party.insert(invitee, party_id);

        Ok(party_id)
    }

    /// Removes a player from their party. Disbands the party if empty.
    pub fn leave_party(&mut self, player: Guid) -> Result<bool, &'static str> {
        let party_id = *self
            .player_to_party
            .get(&player)
            .ok_or("Player is not in a party")?;

        let party = self.parties.get_mut(&party_id).ok_or("Party not found")?;
        let disbanded = party.remove_member(player)?;

        self.player_to_party.remove(&player);

        if disbanded {
            self.parties.remove(&party_id);
        }

        Ok(disbanded)
    }
}

/// Standalone plugin for party grouping and roster coordination.
pub struct PartyPlugin;

impl GameplayPlugin for PartyPlugin {
    fn id(&self) -> &'static str {
        "party"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<PartyMemberComponent>("party_member");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    #[test]
    fn test_party_lifecycle_invite_and_leadership_handover() {
        let p1 = Guid::new(EntityType::Player, 1, 10);
        let p2 = Guid::new(EntityType::Player, 1, 20);

        let mut mgr = PartyManager::new();
        let party_id = mgr.create_party(p1).unwrap();

        // P1 invites P2
        assert!(mgr.invite_player(p1, p2).is_ok());

        // P2 accepts
        assert_eq!(mgr.accept_invite(p2).unwrap(), party_id);

        let party = mgr.get_party(party_id).unwrap();
        assert_eq!(party.members.len(), 2);
        assert_eq!(party.leader, p1);

        // P1 leaves -> leadership automatically passes to P2
        let disbanded = mgr.leave_party(p1).unwrap();
        assert!(!disbanded);

        let party_after = mgr.get_party(party_id).unwrap();
        assert_eq!(party_after.members.len(), 1);
        assert_eq!(party_after.leader, p2);

        // P2 leaves -> party disbands
        let disbanded = mgr.leave_party(p2).unwrap();
        assert!(disbanded);
        assert!(mgr.get_party(party_id).is_none());
    }
}
