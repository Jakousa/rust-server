use mmo_core::guid::{EntityType, Guid};
use mmo_core::octree::{BoundingBox3D, Octree};
use proptest::prelude::*;
use std::collections::HashSet;

prop_compose! {
    fn arb_point()(
        x in -900.0f32..900.0f32,
        y in -900.0f32..900.0f32,
        z in -900.0f32..900.0f32,
    ) -> (f32, f32, f32) {
        (x, y, z)
    }
}

prop_compose! {
    fn arb_bounding_box()(
        p1 in arb_point(),
        p2 in arb_point(),
    ) -> BoundingBox3D {
        BoundingBox3D::new(p1, p2)
    }
}

prop_compose! {
    fn arb_entity_entry()(
        node_id in 1u16..50u16,
        sequence in 1u32..10_000u32,
        pos in arb_point(),
    ) -> (Guid, (f32, f32, f32)) {
        (Guid::new(EntityType::Player, node_id, sequence), pos)
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn prop_test_bounding_box_contains_center(bbox in arb_bounding_box()) {
        let center = bbox.center();
        prop_assert!(bbox.contains(center));
    }

    #[test]
    fn prop_test_bounding_box_intersection_commutativity(b1 in arb_bounding_box(), b2 in arb_bounding_box()) {
        prop_assert_eq!(b1.intersects(&b2), b2.intersects(&b1));
    }

    #[test]
    fn prop_test_bounding_box_intersects_sphere_contains_center(bbox in arb_bounding_box(), radius in 0.0f32..500.0f32) {
        let center = bbox.center();
        prop_assert!(bbox.intersects_sphere(center, radius));
    }

    #[test]
    fn prop_test_octree_radius_query_against_linear_oracle(
        entities in proptest::collection::vec(arb_entity_entry(), 10..80),
        query_center in arb_point(),
        query_radius in 5.0f32..300.0f32,
    ) {
        let world_bounds = BoundingBox3D::new((-1000.0, -1000.0, -1000.0), (1000.0, 1000.0, 1000.0));
        let mut octree = Octree::new(world_bounds);

        // Deduplicate GUIDs by using a map
        let mut deduplicated: Vec<(Guid, (f32, f32, f32))> = Vec::new();
        let mut seen = HashSet::new();
        for (guid, pos) in entities {
            if seen.insert(guid) {
                deduplicated.push((guid, pos));
                octree.insert(guid, pos);
            }
        }

        // Oracle: exact linear Euclidean distance filter
        let r_sq = query_radius * query_radius;
        let oracle_set: HashSet<Guid> = deduplicated
            .into_iter()
            .filter(|(_, pos)| {
                let dx = pos.0 - query_center.0;
                let dy = pos.1 - query_center.1;
                let dz = pos.2 - query_center.2;
                dx * dx + dy * dy + dz * dz <= r_sq
            })
            .map(|(guid, _)| guid)
            .collect();

        // Optimized Octree spatial query
        let octree_results = octree.query_radius(query_center, query_radius);
        let octree_set: HashSet<Guid> = octree_results.into_iter().collect();

        // Must match exactly: no false positives, no false negatives
        prop_assert_eq!(octree_set, oracle_set);
    }

    #[test]
    fn prop_test_octree_insert_remove_symmetry(
        entry in arb_entity_entry(),
    ) {
        let world_bounds = BoundingBox3D::new((-1000.0, -1000.0, -1000.0), (1000.0, 1000.0, 1000.0));
        let mut octree = Octree::new(world_bounds);

        let (guid, pos) = entry;
        octree.insert(guid, pos);
        let found_before = octree.query_radius(pos, 1.0);
        prop_assert!(found_before.contains(&guid));

        let removed = octree.remove(guid, pos);
        prop_assert!(removed);

        let found_after = octree.query_radius(pos, 1.0);
        prop_assert!(!found_after.contains(&guid));
    }

    #[test]
    fn prop_test_octree_len_invariant_against_oracle(
        entities in proptest::collection::vec(arb_entity_entry(), 10..100),
    ) {
        let world_bounds = BoundingBox3D::new((-1000.0, -1000.0, -1000.0), (1000.0, 1000.0, 1000.0));
        let mut octree = Octree::new(world_bounds);

        let mut inserted = HashSet::new();
        for (guid, pos) in &entities {
            if inserted.insert(*guid) {
                octree.insert(*guid, *pos);
            }
            prop_assert_eq!(octree.len(), inserted.len());
        }

        let to_remove: Vec<(Guid, (f32, f32, f32))> =
            entities.iter().take(entities.len() / 2).copied().collect();
        for (guid, pos) in to_remove {
            if inserted.remove(&guid) {
                octree.remove(guid, pos);
            }
            prop_assert_eq!(octree.len(), inserted.len());
        }

        octree.clear();
        prop_assert_eq!(octree.len(), 0);
        prop_assert!(octree.is_empty());
    }
}
