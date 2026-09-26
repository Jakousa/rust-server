use bytes::{Bytes, BytesMut};
use mmo_schema::client::{ClientPacket, InteractionPacket, InteractionType, MovementPacket, Vec3};
use proptest::prelude::*;

prop_compose! {
    fn arb_vec3()(
        x in -10000.0f32..10000.0f32,
        y in -10000.0f32..10000.0f32,
        z in -10000.0f32..10000.0f32,
    ) -> Vec3 {
        Vec3::new(x, y, z)
    }
}

prop_compose! {
    fn arb_interaction_type()(raw in 0u8..=4) -> InteractionType {
        InteractionType::from(raw)
    }
}

prop_compose! {
    fn arb_movement_packet()(
        entity_id in any::<u64>(),
        pos in arb_vec3(),
        vel in arb_vec3(),
        yaw in -std::f32::consts::PI..std::f32::consts::PI,
        pitch in -std::f32::consts::PI..std::f32::consts::PI,
        timestamp in any::<u64>(),
        sequence in any::<u32>(),
    ) -> MovementPacket {
        MovementPacket {
            entity_id,
            position: pos,
            velocity: vel,
            yaw,
            pitch,
            timestamp,
            sequence,
        }
    }
}

prop_compose! {
    fn arb_interaction_packet()(
        source_guid in any::<u64>(),
        target_guid in any::<u64>(),
        interaction_type in arb_interaction_type(),
        action_id in any::<u32>(),
        payload_vec in proptest::collection::vec(any::<u8>(), 0..4096),
    ) -> InteractionPacket {
        InteractionPacket {
            source_guid,
            target_guid,
            interaction_type,
            action_id,
            payload: Bytes::from(payload_vec),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn prop_test_movement_packet_roundtrip(packet in arb_movement_packet()) {
        let client_packet = ClientPacket::Movement(packet.clone());
        let encoded = client_packet.encode();
        let mut buf = BytesMut::from(&encoded[..]);

        let decoded = ClientPacket::decode_frame(&mut buf)
            .expect("Valid frame must decode without error")
            .expect("Complete frame must yield Some packet");

        match decoded {
            ClientPacket::Movement(decoded_move) => {
                prop_assert_eq!(decoded_move.entity_id, packet.entity_id);
                prop_assert_eq!(decoded_move.sequence, packet.sequence);
                prop_assert_eq!(decoded_move.timestamp, packet.timestamp);
                prop_assert_eq!(decoded_move.position, packet.position);
                prop_assert_eq!(decoded_move.velocity, packet.velocity);
                prop_assert_eq!(decoded_move.yaw.to_bits(), packet.yaw.to_bits());
                prop_assert_eq!(decoded_move.pitch.to_bits(), packet.pitch.to_bits());
            }
            other => panic!("Expected Movement packet, got {:?}", other),
        }
    }

    #[test]
    fn prop_test_interaction_packet_arbitrary_payload_roundtrip(packet in arb_interaction_packet()) {
        let client_packet = ClientPacket::Interaction(packet.clone());
        let encoded = client_packet.encode();
        let mut buf = BytesMut::from(&encoded[..]);

        let decoded = ClientPacket::decode_frame(&mut buf)
            .expect("Valid frame must decode without error")
            .expect("Complete frame must yield Some packet");

        match decoded {
            ClientPacket::Interaction(decoded_interaction) => {
                prop_assert_eq!(decoded_interaction.source_guid, packet.source_guid);
                prop_assert_eq!(decoded_interaction.target_guid, packet.target_guid);
                prop_assert_eq!(decoded_interaction.interaction_type, packet.interaction_type);
                prop_assert_eq!(decoded_interaction.action_id, packet.action_id);
                prop_assert_eq!(decoded_interaction.payload, packet.payload);
            }
            other => panic!("Expected Interaction packet, got {:?}", other),
        }
    }

    #[test]
    fn prop_test_decode_frame_fuzz_never_panics(fuzz_bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut buf = BytesMut::from(&fuzz_bytes[..]);
        // Function must never panic under arbitrary random byte stream
        let _ = ClientPacket::decode_frame(&mut buf);
    }

    #[test]
    fn prop_test_chunked_streaming_delivery(packet in arb_movement_packet(), split_point in 1usize..20) {
        let client_packet = ClientPacket::Movement(packet.clone());
        let encoded = client_packet.encode();

        let total_len = encoded.len();
        let split_at = split_point.min(total_len - 1);

        let mut stream_buf = BytesMut::new();
        stream_buf.extend_from_slice(&encoded[..split_at]);

        // First partial chunk should return Ok(None)
        let first_result = ClientPacket::decode_frame(&mut stream_buf)
            .expect("Valid partial chunk must not produce decoding error");
        prop_assert!(first_result.is_none());

        // Add remaining chunk
        stream_buf.extend_from_slice(&encoded[split_at..]);
        let second_result = ClientPacket::decode_frame(&mut stream_buf)
            .expect("Frame must decode successfully once all bytes arrive");

        prop_assert!(second_result.is_some());
    }
}
