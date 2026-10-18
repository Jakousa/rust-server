//! Universal Container and Slot domain abstractions for generic asset and inventory management.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Strongly-typed 64-bit container identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContainerId(pub u64);

impl ContainerId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u64> for ContainerId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<ContainerId> for u64 {
    fn from(id: ContainerId) -> Self {
        id.0
    }
}

impl fmt::Display for ContainerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Logical owner of a container.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContainerOwner {
    Character(u64),
    Guild(u64),
    Group(u64),
    WorldEntity(u64),
    System(String),
}

impl ContainerOwner {
    /// Returns the database-persisted owner type string.
    pub fn owner_type(&self) -> &'static str {
        match self {
            Self::Character(_) => "character",
            Self::Guild(_) => "guild",
            Self::Group(_) => "group",
            Self::WorldEntity(_) => "world_entity",
            Self::System(_) => "system",
        }
    }

    /// Returns the database-persisted owner identifier string.
    pub fn owner_id(&self) -> String {
        match self {
            Self::Character(id) | Self::Guild(id) | Self::Group(id) | Self::WorldEntity(id) => {
                id.to_string()
            }
            Self::System(s) => s.clone(),
        }
    }

    /// Parses database owner fields into a `ContainerOwner`.
    pub fn from_parts(owner_type: &str, owner_id: &str) -> Result<Self, ContainerError> {
        match owner_type {
            "character" => {
                let id = owner_id.parse::<u64>().map_err(|_| {
                    ContainerError::InvalidOwner(owner_type.to_string(), owner_id.to_string())
                })?;
                Ok(Self::Character(id))
            }
            "guild" => {
                let id = owner_id.parse::<u64>().map_err(|_| {
                    ContainerError::InvalidOwner(owner_type.to_string(), owner_id.to_string())
                })?;
                Ok(Self::Guild(id))
            }
            "group" => {
                let id = owner_id.parse::<u64>().map_err(|_| {
                    ContainerError::InvalidOwner(owner_type.to_string(), owner_id.to_string())
                })?;
                Ok(Self::Group(id))
            }
            "world_entity" => {
                let id = owner_id.parse::<u64>().map_err(|_| {
                    ContainerError::InvalidOwner(owner_type.to_string(), owner_id.to_string())
                })?;
                Ok(Self::WorldEntity(id))
            }
            "system" => Ok(Self::System(owner_id.to_string())),
            _ => Err(ContainerError::InvalidOwner(
                owner_type.to_string(),
                owner_id.to_string(),
            )),
        }
    }
}

/// Access control policies governing who may view or modify items in a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerAccessPolicy {
    OwnerOnly,
    PublicReadWrite,
    PublicReadOwnerWrite,
    RoleRestricted { min_role: u32 },
}

impl ContainerAccessPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OwnerOnly => "owner_only",
            Self::PublicReadWrite => "public_read_write",
            Self::PublicReadOwnerWrite => "public_read_owner_write",
            Self::RoleRestricted { .. } => "role_restricted",
        }
    }

    pub fn from_str_parts(policy_str: &str, min_role: Option<u32>) -> Result<Self, ContainerError> {
        match policy_str {
            "owner_only" => Ok(Self::OwnerOnly),
            "public_read_write" => Ok(Self::PublicReadWrite),
            "public_read_owner_write" => Ok(Self::PublicReadOwnerWrite),
            "role_restricted" => Ok(Self::RoleRestricted {
                min_role: min_role.unwrap_or(0),
            }),
            other => Err(ContainerError::InvalidPolicy(other.to_string())),
        }
    }

    /// Evaluates read permissions for an accessor against the container owner.
    pub fn can_read(
        &self,
        container_owner: &ContainerOwner,
        accessor: &ContainerOwner,
        accessor_role: Option<u32>,
    ) -> bool {
        match self {
            Self::OwnerOnly => container_owner == accessor,
            Self::PublicReadWrite | Self::PublicReadOwnerWrite => true,
            Self::RoleRestricted { min_role } => {
                if container_owner == accessor {
                    true
                } else {
                    accessor_role.is_some_and(|r| r >= *min_role)
                }
            }
        }
    }

    /// Evaluates write permissions for an accessor against the container owner.
    pub fn can_write(
        &self,
        container_owner: &ContainerOwner,
        accessor: &ContainerOwner,
        accessor_role: Option<u32>,
    ) -> bool {
        match self {
            Self::OwnerOnly | Self::PublicReadOwnerWrite => container_owner == accessor,
            Self::PublicReadWrite => true,
            Self::RoleRestricted { min_role } => {
                if container_owner == accessor {
                    true
                } else {
                    accessor_role.is_some_and(|r| r >= *min_role)
                }
            }
        }
    }
}

/// Zero-indexed slot position within a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContainerSlot(pub u32);

impl ContainerSlot {
    pub const fn new(slot: u32) -> Self {
        Self(slot)
    }

    pub const fn as_u32(&self) -> u32 {
        self.0
    }

    /// Validates that this slot position is strictly less than container capacity.
    pub fn validate_capacity(&self, capacity: u32) -> Result<(), ContainerError> {
        if self.0 >= capacity {
            Err(ContainerError::SlotOutOfBounds {
                slot: self.0,
                capacity,
            })
        } else {
            Ok(())
        }
    }
}

impl From<u32> for ContainerSlot {
    fn from(s: u32) -> Self {
        Self(s)
    }
}

impl From<ContainerSlot> for u32 {
    fn from(s: ContainerSlot) -> Self {
        s.0
    }
}

/// Errors originating from container domain rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContainerError {
    #[error("Slot index {slot} exceeds container capacity {capacity}")]
    SlotOutOfBounds { slot: u32, capacity: u32 },

    #[error("Container is at full capacity ({capacity})")]
    CapacityExceeded { capacity: u32 },

    #[error("Access denied for container {container_id} under policy {policy:?}")]
    AccessDenied {
        container_id: u64,
        policy: ContainerAccessPolicy,
    },

    #[error("Invalid container owner representation: type={0}, id={1}")]
    InvalidOwner(String, String),

    #[error("Invalid container access policy string: {0}")]
    InvalidPolicy(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_id_conversions() {
        let cid = ContainerId::new(42);
        assert_eq!(cid.as_u64(), 42);
        assert_eq!(u64::from(cid), 42);
        assert_eq!(ContainerId::from(42), cid);
        assert_eq!(cid.to_string(), "42");
    }

    #[test]
    fn test_container_owner_serialization_and_parsing() {
        let char_owner = ContainerOwner::Character(1001);
        assert_eq!(char_owner.owner_type(), "character");
        assert_eq!(char_owner.owner_id(), "1001");
        assert_eq!(
            ContainerOwner::from_parts("character", "1001").unwrap(),
            char_owner
        );

        let guild_owner = ContainerOwner::Guild(50);
        assert_eq!(guild_owner.owner_type(), "guild");
        assert_eq!(guild_owner.owner_id(), "50");
        assert_eq!(
            ContainerOwner::from_parts("guild", "50").unwrap(),
            guild_owner
        );

        let group_owner = ContainerOwner::Group(75);
        assert_eq!(group_owner.owner_type(), "group");
        assert_eq!(group_owner.owner_id(), "75");
        assert_eq!(
            ContainerOwner::from_parts("group", "75").unwrap(),
            group_owner
        );

        let sys_owner = ContainerOwner::System("escrow_pool".to_string());
        assert_eq!(sys_owner.owner_type(), "system");
        assert_eq!(sys_owner.owner_id(), "escrow_pool");
        assert_eq!(
            ContainerOwner::from_parts("system", "escrow_pool").unwrap(),
            sys_owner
        );

        assert!(ContainerOwner::from_parts("character", "not_a_number").is_err());
        assert!(ContainerOwner::from_parts("unknown_type", "123").is_err());
    }

    #[test]
    fn test_access_policy_permissions() {
        let owner = ContainerOwner::Character(100);
        let stranger = ContainerOwner::Character(200);

        // OwnerOnly
        let policy = ContainerAccessPolicy::OwnerOnly;
        assert!(policy.can_read(&owner, &owner, None));
        assert!(policy.can_write(&owner, &owner, None));
        assert!(!policy.can_read(&owner, &stranger, None));
        assert!(!policy.can_write(&owner, &stranger, None));

        // PublicReadOwnerWrite
        let policy = ContainerAccessPolicy::PublicReadOwnerWrite;
        assert!(policy.can_read(&owner, &stranger, None));
        assert!(!policy.can_write(&owner, &stranger, None));
        assert!(policy.can_write(&owner, &owner, None));

        // PublicReadWrite
        let policy = ContainerAccessPolicy::PublicReadWrite;
        assert!(policy.can_read(&owner, &stranger, None));
        assert!(policy.can_write(&owner, &stranger, None));

        // RoleRestricted
        let policy = ContainerAccessPolicy::RoleRestricted { min_role: 3 };
        assert!(policy.can_read(&owner, &owner, None));
        assert!(policy.can_write(&owner, &owner, None));
        assert!(!policy.can_read(&owner, &stranger, Some(2)));
        assert!(!policy.can_write(&owner, &stranger, Some(2)));
        assert!(policy.can_read(&owner, &stranger, Some(3)));
        assert!(policy.can_write(&owner, &stranger, Some(3)));
        assert!(policy.can_write(&owner, &stranger, Some(5)));
    }

    #[test]
    fn test_slot_capacity_validation() {
        let slot = ContainerSlot::new(5);
        assert!(slot.validate_capacity(10).is_ok());
        assert!(slot.validate_capacity(6).is_ok());
        assert_eq!(
            slot.validate_capacity(5),
            Err(ContainerError::SlotOutOfBounds {
                slot: 5,
                capacity: 5
            })
        );
        assert_eq!(
            slot.validate_capacity(3),
            Err(ContainerError::SlotOutOfBounds {
                slot: 5,
                capacity: 3
            })
        );
    }
}
