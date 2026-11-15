## ADDED Requirements

### Requirement: Automatic Underloaded Partition Merging and Rebalancing
`mmo-director` SHALL automatically detect underloaded 8-leaf split branches during heartbeat evaluation and collapse them back onto a single surviving node, propagating consolidated partition maps so entities migrate back onto the consolidated owner.

#### Scenario: Underloaded split branch merges back onto surviving node
- **GIVEN** a world partition was previously split into 8 child leaves distributed across two active nodes `Node 1` and `Node 2`
- **AND** the `split_cooldown` period has elapsed and no child owner is tick-saturated
- **WHEN** the combined distinct-owner entity count across the branch's 8 leaves drops at or below `merge_threshold` and `Node 1` sends a `Heartbeat`
- **THEN** `mmo-director` SHALL collapse the 8 child leaves into the parent leaf assigned to the lowest-load surviving owner via `handle_merge`
- **AND** increment `topology_epoch`
- **AND** return the consolidated partition list in `HeartbeatResponse.partitions` so peer nodes migrate entities back to the consolidated owner.

#### Scenario: Split cooldown prevents premature merge oscillation
- **GIVEN** a split occurred within `split_cooldown` on a branch owner
- **WHEN** `evaluate_and_merge_underloaded` evaluates that branch
- **THEN** `mmo-director` SHALL defer merging that branch until the cooldown elapses.
