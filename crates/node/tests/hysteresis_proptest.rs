//! Property-based testing for boundary crossing detection, hysteresis dampening,
//! and migration cooldowns across arbitrary coordinate spaces.

use mmo_core::guid::{EntityType, Guid};
use mmo_core::BoundingBox3D;
use mmo_node::TickSimulation;
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_test_inside_bounds_never_triggers_migration(
        x in -100.0f32..100.0f32,
        y in -100.0f32..100.0f32,
        z in -100.0f32..100.0f32,
    ) {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 1);

        let pos = (x, y, z);
        prop_assert!(!sim.check_boundary_migration(guid, pos, 1.0, 20));
    }

    #[test]
    fn prop_test_sub_margin_outside_never_triggers_migration(
        // Outside along X axis by distance delta in [0.01, 0.99]
        delta in 0.01f32..0.99f32,
        y in -45.0f32..45.0f32,
        z in -45.0f32..45.0f32,
    ) {
        let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
        let sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 2);

        let pos = (50.0 + delta, y, z);
        prop_assert!(!sim.check_boundary_migration(guid, pos, 1.0, 20));
    }

    #[test]
    fn prop_test_beyond_margin_outside_triggers_migration_when_not_in_cooldown(
        // Outside along X axis by distance delta >= 1.0
        delta in 1.05f32..20.0f32,
        y in -40.0f32..40.0f32,
        z in -40.0f32..40.0f32,
    ) {
        let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
        let sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 3);

        let pos = (50.0 + delta, y, z);
        prop_assert!(sim.check_boundary_migration(guid, pos, 1.0, 20));
    }

    #[test]
    fn prop_test_cooldown_strictly_suppresses_migration(
        delta in 1.05f32..50.0f32,
        cooldown_ticks in 1u64..100u64,
        tick_offset in 0u64..100u64,
    ) {
        let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
        let mut sim = TickSimulation::new(1, bounds);
        let guid = Guid::new(EntityType::Player, 1, 4);

        let pos = (50.0 + delta, 0.0, 0.0);

        // Set cooldown at current tick
        sim.set_last_migration_tick(guid, sim.tick_counter);

        if tick_offset < cooldown_ticks {
            // Still within cooldown
            sim.tick_counter += tick_offset;
            prop_assert!(sim.is_in_migration_cooldown(guid, cooldown_ticks));
            prop_assert!(!sim.check_boundary_migration(guid, pos, 1.0, cooldown_ticks));
        } else {
            // Cooldown elapsed
            sim.tick_counter += cooldown_ticks + 1;
            prop_assert!(!sim.is_in_migration_cooldown(guid, cooldown_ticks));
            prop_assert!(sim.check_boundary_migration(guid, pos, 1.0, cooldown_ticks));
        }
    }
}
