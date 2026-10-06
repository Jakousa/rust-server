use bytes::{Bytes, BytesMut};
use mmo_schema::client::{
    AuthRequestPacket, AuthResponsePacket, ClientPacket, InteractionPacket, InteractionType,
    MovementPacket, Vec3,
};
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

prop_compose! {
    fn arb_auth_request_packet()(
        token in "[a-zA-Z0-9_.-]{10,200}",
        client_version in any::<u32>(),
    ) -> AuthRequestPacket {
        AuthRequestPacket {
            token,
            client_version,
        }
    }
}

prop_compose! {
    fn arb_auth_response_packet()(
        success in any::<bool>(),
        error_code in 0u32..10,
        session_id in any::<u64>(),
        player_guid in any::<u64>(),
        assigned_node in "[a-z0-9.:]{5,30}",
        error_message in "[a-zA-Z0-9 ]{0,50}",
    ) -> AuthResponsePacket {
        AuthResponsePacket {
            success,
            error_code,
            session_id,
            player_guid,
            assigned_node,
            error_message,
        }
    }
}

prop_compose! {
    fn arb_social_message_packet()(
        channel_type in 0u8..=5,
        target_id in any::<u64>(),
        sender_guid in any::<u64>(),
        sender_name in "[a-zA-Z0-9]{1,24}",
        content in "[a-zA-Z0-9 ]{0,100}",
        timestamp in any::<u64>(),
    ) -> mmo_schema::client::SocialMessagePacket {
        mmo_schema::client::SocialMessagePacket {
            channel_type,
            target_id,
            sender_guid,
            sender_name,
            content,
            timestamp,
        }
    }
}

prop_compose! {
    fn arb_market_buy_packet()(
        listing_id in "[a-zA-Z0-9-]{1,24}",
        buyer_container_id in any::<u64>(),
        payment_container_id in any::<u64>(),
        price in any::<u64>(),
    ) -> mmo_schema::client::MarketBuyRequestPacket {
        mmo_schema::client::MarketBuyRequestPacket {
            listing_id,
            buyer_container_id,
            payment_container_id,
            price,
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
    fn prop_test_auth_request_roundtrip(packet in arb_auth_request_packet()) {
        let client_packet = ClientPacket::AuthRequest(packet.clone());
        let encoded = client_packet.encode();
        let mut buf = BytesMut::from(&encoded[..]);

        let decoded = ClientPacket::decode_frame(&mut buf)
            .expect("Valid frame must decode without error")
            .expect("Complete frame must yield Some packet");

        match decoded {
            ClientPacket::AuthRequest(decoded_auth) => {
                prop_assert_eq!(decoded_auth.token, packet.token);
                prop_assert_eq!(decoded_auth.client_version, packet.client_version);
            }
            other => panic!("Expected AuthRequest packet, got {:?}", other),
        }
    }

    #[test]
    fn prop_test_auth_response_roundtrip(packet in arb_auth_response_packet()) {
        let client_packet = ClientPacket::AuthResponse(packet.clone());
        let encoded = client_packet.encode();
        let mut buf = BytesMut::from(&encoded[..]);

        let decoded = ClientPacket::decode_frame(&mut buf)
            .expect("Valid frame must decode without error")
            .expect("Complete frame must yield Some packet");

        match decoded {
            ClientPacket::AuthResponse(decoded_auth) => {
                prop_assert_eq!(decoded_auth.success, packet.success);
                prop_assert_eq!(decoded_auth.error_code, packet.error_code);
                prop_assert_eq!(decoded_auth.session_id, packet.session_id);
                prop_assert_eq!(decoded_auth.player_guid, packet.player_guid);
                prop_assert_eq!(decoded_auth.assigned_node, packet.assigned_node);
                prop_assert_eq!(decoded_auth.error_message, packet.error_message);
            }
            other => panic!("Expected AuthResponse packet, got {:?}", other),
        }
    }

    #[test]
    fn prop_test_social_message_roundtrip(packet in arb_social_message_packet()) {
        let client_packet = ClientPacket::SocialMessage(packet.clone());
        let encoded = client_packet.encode();
        let mut buf = BytesMut::from(&encoded[..]);

        let decoded = ClientPacket::decode_frame(&mut buf)
            .expect("Valid frame must decode without error")
            .expect("Complete frame must yield Some packet");

        match decoded {
            ClientPacket::SocialMessage(dec) => {
                prop_assert_eq!(dec.channel_type, packet.channel_type);
                prop_assert_eq!(dec.target_id, packet.target_id);
                prop_assert_eq!(dec.sender_guid, packet.sender_guid);
                prop_assert_eq!(dec.sender_name, packet.sender_name);
                prop_assert_eq!(dec.content, packet.content);
                prop_assert_eq!(dec.timestamp, packet.timestamp);
            }
            other => panic!("Expected SocialMessage packet, got {:?}", other),
        }
    }

    #[test]
    fn prop_test_market_buy_roundtrip(packet in arb_market_buy_packet()) {
        let client_packet = ClientPacket::MarketBuyRequest(packet.clone());
        let encoded = client_packet.encode();
        let mut buf = BytesMut::from(&encoded[..]);

        let decoded = ClientPacket::decode_frame(&mut buf)
            .expect("Valid frame must decode without error")
            .expect("Complete frame must yield Some packet");

        match decoded {
            ClientPacket::MarketBuyRequest(dec) => {
                prop_assert_eq!(dec.listing_id, packet.listing_id);
                prop_assert_eq!(dec.buyer_container_id, packet.buyer_container_id);
                prop_assert_eq!(dec.payment_container_id, packet.payment_container_id);
                prop_assert_eq!(dec.price, packet.price);
            }
            other => panic!("Expected MarketBuyRequest packet, got {:?}", other),
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

    #[test]
    fn prop_test_quantize_delta_roundtrip(val in -254.9f32..254.9f32) {
        let q = mmo_schema::quantize::quantize_delta_i16(val);
        let reconstructed = mmo_schema::quantize::dequantize_delta_i16(q);
        let error = (val - reconstructed).abs();
        prop_assert!(error <= 0.008, "Error {} exceeded 8mm tolerance for {}", error, val);
    }

    #[test]
    fn prop_test_quantize_yaw_roundtrip(yaw in -100.0f32..100.0f32) {
        let q = mmo_schema::quantize::quantize_yaw_u8(yaw);
        let reconstructed = mmo_schema::quantize::dequantize_yaw_u8(q);
        let diff = (yaw.rem_euclid(2.0 * std::f32::consts::PI) - reconstructed).abs();
        let angular_error = diff.min((2.0 * std::f32::consts::PI - diff).abs());
        let max_error_rad = (1.5 / 180.0) * std::f32::consts::PI;
        prop_assert!(angular_error <= max_error_rad, "Yaw error {} exceeded 1.5 deg for {}", angular_error, yaw);
    }

    #[test]
    fn prop_test_quantize_never_panics(fuzz_val in any::<f32>()) {
        let _ = mmo_schema::quantize::quantize_delta_i16(fuzz_val);
        let _ = mmo_schema::quantize::quantize_yaw_u8(fuzz_val);
        let _ = mmo_schema::quantize::quantize_pitch_i8(fuzz_val);
        let _ = mmo_schema::quantize::quantize_velocity_i16(fuzz_val);
    }

    #[test]
    fn prop_test_varint_u64_roundtrip(val in any::<u64>()) {
        let mut buf = bytes::BytesMut::new();
        mmo_schema::bitpack::put_varint_u64(&mut buf, val);
        let decoded = mmo_schema::bitpack::get_varint_u64(&mut buf).unwrap();
        prop_assert_eq!(val, decoded);
    }
}
