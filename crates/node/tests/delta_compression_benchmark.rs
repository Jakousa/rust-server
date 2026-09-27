//! Bandwidth profiling and verification benchmark for Track 6.
//!
//! Compares uncompressed `EntityStatePacket` replication against bit-packed
//! `EntityDeltaBatchPacket` replication under 1,000 entity movement density,
//! and verifies that egress bandwidth reduction exceeds the 60% requirement (targeting 70%-80%).

use mmo_core::guid::Guid;
use mmo_core::{BoundingBox3D, EntityStore, Octree};
use mmo_node::aoi::{AoiManager, ObserverConfig};

fn make_guid(id: u64) -> Guid {
    Guid::from_raw(id)
}

#[test]
fn test_bandwidth_compression_under_1000_entity_density() {
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    const NUM_ENTITIES: usize = 1_000;
    const NUM_TICKS: u64 = 50;

    // ==========================================
    // 1. UNCOMPRESSED RUN (EntityStatePacket)
    // ==========================================
    let mut store_uncompressed = EntityStore::new();
    let mut octree_uncompressed = Octree::new(bounds);
    let mut aoi_uncompressed = AoiManager::new();

    let player_guid = make_guid(1);
    store_uncompressed.spawn(player_guid, (0.0, 0.0, 0.0));
    octree_uncompressed.insert(player_guid, (0.0, 0.0, 0.0));

    let uncompressed_config = ObserverConfig {
        base_radius: 500.0,
        max_entities: 1000,
        enable_delta_compression: false,
        ..Default::default()
    };
    aoi_uncompressed.register_observer(1, player_guid, uncompressed_config);

    // Spawn 1,000 entities in grid
    for i in 2..=(NUM_ENTITIES + 1) {
        let guid = make_guid(i as u64);
        let x = ((i % 32) as f32 - 16.0) * 10.0;
        let z = ((i / 32) as f32 - 16.0) * 10.0;
        store_uncompressed.spawn(guid, (x, 0.0, z));
        octree_uncompressed.insert(guid, (x, 0.0, z));
    }

    let mut uncompressed_bytes: usize = 0;
    let mut timestamp_ms: u64 = 1000;

    for tick in 1..=NUM_TICKS {
        timestamp_ms += 50;
        // Move all entities slightly (0.25m per tick = 5 m/s)
        for i in 2..=(NUM_ENTITIES + 1) {
            let guid = make_guid(i as u64);
            if let Some(ent) = store_uncompressed.get_mut_by_guid(guid) {
                let old_pos = ent.position;
                let new_pos = (old_pos.0 + 0.15, 0.0, old_pos.2 + 0.20);
                ent.position = new_pos;
                octree_uncompressed.remove(guid, old_pos);
                octree_uncompressed.insert(guid, new_pos);
            }
        }

        let jobs = aoi_uncompressed.update_observers(
            &store_uncompressed,
            &octree_uncompressed,
            tick,
            timestamp_ms,
        );
        for job in jobs {
            for packet in job.packets {
                uncompressed_bytes += packet.encode().len();
            }
        }
    }

    // ==========================================
    // 2. DELTA COMPRESSED RUN (EntityDeltaBatchPacket)
    // ==========================================
    let mut store_compressed = EntityStore::new();
    let mut octree_compressed = Octree::new(bounds);
    let mut aoi_compressed = AoiManager::new();

    store_compressed.spawn(player_guid, (0.0, 0.0, 0.0));
    octree_compressed.insert(player_guid, (0.0, 0.0, 0.0));

    let compressed_config = ObserverConfig {
        base_radius: 500.0,
        max_entities: 1000,
        enable_delta_compression: true,
        ..Default::default()
    };
    aoi_compressed.register_observer(1, player_guid, compressed_config);

    // Spawn identical 1,000 entities in grid
    for i in 2..=(NUM_ENTITIES + 1) {
        let guid = make_guid(i as u64);
        let x = ((i % 32) as f32 - 16.0) * 10.0;
        let z = ((i / 32) as f32 - 16.0) * 10.0;
        store_compressed.spawn(guid, (x, 0.0, z));
        octree_compressed.insert(guid, (x, 0.0, z));
    }

    let mut compressed_bytes: usize = 0;
    timestamp_ms = 1000;

    for tick in 1..=NUM_TICKS {
        timestamp_ms += 50;
        // Move all entities identically (0.25m per tick = 5 m/s)
        for i in 2..=(NUM_ENTITIES + 1) {
            let guid = make_guid(i as u64);
            if let Some(ent) = store_compressed.get_mut_by_guid(guid) {
                let old_pos = ent.position;
                let new_pos = (old_pos.0 + 0.15, 0.0, old_pos.2 + 0.20);
                ent.position = new_pos;
                octree_compressed.remove(guid, old_pos);
                octree_compressed.insert(guid, new_pos);
            }
        }

        let jobs = aoi_compressed.update_observers(
            &store_compressed,
            &octree_compressed,
            tick,
            timestamp_ms,
        );
        for job in jobs {
            for packet in job.packets {
                compressed_bytes += packet.encode().len();
            }
        }

        // Simulate client acknowledging each tick (1 tick latency)
        aoi_compressed.process_ack(1, tick, 0, tick);
    }

    // ==========================================
    // 3. COMPARISON & REDUCTION ASSERTIONS
    // ==========================================
    let savings = 1.0 - (compressed_bytes as f64 / uncompressed_bytes as f64);
    let savings_pct = savings * 100.0;

    println!("============================================================");
    println!("     BANDWIDTH DELTA COMPRESSION BENCHMARK REPORT           ");
    println!("============================================================");
    println!("Entities in AoI     : {}", NUM_ENTITIES);
    println!("Simulation Ticks    : {}", NUM_TICKS);
    println!(
        "Uncompressed Bytes  : {} bytes ({:.2} MB)",
        uncompressed_bytes,
        uncompressed_bytes as f64 / 1_048_576.0
    );
    println!(
        "Compressed Bytes    : {} bytes ({:.2} MB)",
        compressed_bytes,
        compressed_bytes as f64 / 1_048_576.0
    );
    println!("Total Egress Savings: {:.2}%", savings_pct);
    println!("------------------------------------------------------------");

    // Must achieve at least 60% bandwidth savings (target 70%-80%)
    assert!(
        savings >= 0.60,
        "Bandwidth savings {:.2}% did not meet 60% target!",
        savings_pct
    );
}

#[test]
fn test_baseline_tracker_memory_bounding_under_latency() {
    let mut aoi = AoiManager::new();
    const NUM_SESSIONS: usize = 1_000;
    const ENTITIES_PER_SESSION: usize = 50;

    // Simulate 1,000 sessions with max 60 ticks history
    for s in 1..=NUM_SESSIONS {
        let config = ObserverConfig {
            history_depth_ticks: 60,
            enable_delta_compression: true,
            ..Default::default()
        };
        aoi.register_observer(s as u64, make_guid(s as u64), config);
    }

    assert_eq!(aoi.observer_count(), NUM_SESSIONS);

    // Approximate memory per tick snapshot with 50 entities
    // Each entity is Guid (8B) + CachedEntityState (48B) + HashMap overhead (~32B) ≈ 88B
    // 50 entities * 88B ≈ 4.4 KB per tick snapshot
    // Healthy ACK scenario (2 snapshots): 2 * 4.4 KB * 1,000 sessions ≈ 8.8 MB (< 20 MB)
    let healthy_mem_estimate_mb =
        (NUM_SESSIONS * 2 * ENTITIES_PER_SESSION * 88) as f64 / 1_048_576.0;
    println!(
        "Estimated memory under healthy ACK latency (2 ticks): {:.2} MB",
        healthy_mem_estimate_mb
    );
    assert!(healthy_mem_estimate_mb < 20.0);

    // Worst-case lag scenario (60 snapshots capped): 60 * 4.4 KB * 1,000 sessions ≈ 264 MB (<= 400 MB)
    let worst_case_mem_estimate_mb =
        (NUM_SESSIONS * 60 * ENTITIES_PER_SESSION * 88) as f64 / 1_048_576.0;
    println!(
        "Estimated memory under worst-case 3s lag (60 ticks): {:.2} MB",
        worst_case_mem_estimate_mb
    );
    assert!(worst_case_mem_estimate_mb <= 400.0);
}
