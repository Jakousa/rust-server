# Chaos Simulation & Node Failover Specification

## Purpose
Specifies automated chaos testing, detection of simulation node failure, orphaned spatial partition recovery, and graceful reconciliation of orphaned entities.

## Requirements

### Requirement: Autonomous Node Failure Detection
The Director coordinator MUST monitor heartbeat timestamps from all registered simulation nodes and declare a node dead when its last heartbeat exceeds a threshold $\tau_{\text{dead}} \le 5.0\text{ seconds}$.

#### Scenario: Simulation Node Crash
- **GIVEN** `mmo-node-0` and `mmo-node-1` actively simulating spatial partitions
- **WHEN** `mmo-node-0` process is abruptly killed (`kill -9` or pod deleted)
- **THEN** within $5.0\text{ seconds}$, the Director coordinator identifies `mmo-node-0` as dead and triggers partition recovery.

### Requirement: Orphaned Spatial Partition Reassignment
Upon detecting a dead node, the Director MUST:
1. Traverse the spatial octree and collect all leaf partitions owned by the dead node.
2. Reassign the orphaned partitions to remaining healthy simulation nodes with lowest entity density.
3. Increment the cluster partition epoch.
4. Issue partition reassignment notifications to surviving nodes.

#### Scenario: Partition Leaf Adoption
- **GIVEN** Node 1 owned spatial octant `[(-1000,-1000,-1000), (0,0,0)]` before dying
- **WHEN** Director recovers orphaned leaves
- **THEN** Node 2 is assigned the orphaned octant and assumes simulation authority.

### Requirement: Chaos Automation Script
A shell script `scripts/chaos.sh` MUST automate chaos execution:
1. Verify cluster is healthy.
2. Start or verify bot swarm load.
3. Terminate a target simulation node pod.
4. Assert Director logs report orphaned partition recovery within 5 seconds.
5. Assert Kubernetes StatefulSet automatically resurrects the pod.
6. Assert resurrected pod reconnects and registers with Director.
