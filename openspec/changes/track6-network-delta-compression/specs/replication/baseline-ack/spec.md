# Specification: Baseline Acknowledgement (ACK) Protocol

## Capability: `replication/baseline-ack`

### Context
Delta compression requires sender and receiver to share an identical reference baseline. If deltas are computed relative to a frame the client never received (due to network drops), client state diverges and causes desynchronization. The Baseline ACK protocol ensures deltas are computed relative to states explicitly confirmed received by the client.

---

### Scenario: Client Sends Acknowledgement
GIVEN a client that successfully processed server tick $T_{server}$
WHEN sending a heartbeat or input packet
THEN the client dispatches a `ClientAckPacket` carrying `observed_tick = T_{server}` and an `ack_mask` representing prior 32 ticks
AND transmission consumes $< 16$ bytes on the wire.

### Scenario: Non-Blocking ACK Ingress Queueing
GIVEN incoming `ClientAckPacket`s arriving from multiple gateway connections
WHEN delivered to the simulation node
THEN they are enqueued into a lock-free or MPSC queue without acquiring write locks on the simulation tick loop
AND the simulation tick loop drains and applies all queued ACKs in batch at the start of each 20 Hz tick.

### Scenario: Server Baseline Advancement & Validation
GIVEN an observer with current baseline tick $T_{base}$ and unacknowledged tick buffer
WHEN receiving `ClientAckPacket` with `observed_tick = T_{ack}`
THEN if $T_{ack} > T_{current\_tick}$, the packet is dropped immediately as a future-tick spoofing attempt
AND if $T_{ack} \le T_{current\_tick}$ and $T_{ack} > T_{base}$:
  - The observer's confirmed baseline tick is advanced to $T_{ack}$
  - Entity states stored for ticks $< T_{ack}$ are pruned from the history buffer to reclaim memory
  - Subsequent delta packets are computed relative to the state at $T_{ack}$.

### Scenario: New Entity Enters AoI (Keyframe Initialization)
GIVEN an entity $E$ that just entered an observer's AoI sphere
WHEN generating the next replication batch
THEN entity $E$ is serialized with a full keyframe flag (`MASK_KEYFRAME`)
AND position is sent as absolute coordinates
AND the client establishes its initial baseline for $E$ upon receipt.

### Scenario: Sudden Teleportation or Large Kinematic Displacement
GIVEN an entity whose displacement along any axis $|\Delta x|, |\Delta y|, |\Delta z| > 255.0\text{ meters}$ (e.g. teleportation, respawn, or explosive knockback)
WHEN evaluating delta compression for that entity
THEN the quantizer is bypassed and the entity is automatically serialized with `MASK_KEYFRAME`
AND client receives exact floating-point coordinates without integer saturation or wraparound.

### Scenario: Packet Loss & Baseline Recovery
GIVEN a scenario where network loss prevents ACKs from reaching the server for $> 40$ ticks ($2.0\text{ seconds}$)
WHEN the server generates replication updates
THEN the server detects the stale baseline and automatically elevates the next update to full keyframes
AND client state is resynchronized without disconnecting or requiring world reloads.

### Scenario: Observer Disconnect and Session Drop Cleanup
GIVEN a player session disconnecting or dropping connection at the gateway
WHEN the session drop guard triggers
THEN a cleanup notification is sent to the simulation node
AND `AoiManager::unregister_observer` removes the observer and immediately deallocates its baseline history buffers.
