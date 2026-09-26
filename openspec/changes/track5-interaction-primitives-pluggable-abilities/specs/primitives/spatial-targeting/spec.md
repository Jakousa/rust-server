# Spec: Spatial Targeting & Interaction Queries

## Capability: `primitives/spatial-targeting`

Provides authoritative 3D geometric queries against the simulation Octree for entity targeting, line-of-sight checks, and area-of-effect calculations.

---

### Requirement: Range & Line-of-Sight Validation

#### Scenario: Point-to-point distance check
- **GIVEN** initiator entity $A$ at position $(0, 0, 0)$ and target entity $B$ at position $(15, 0, 0)$
- **WHEN** an interaction with max range $10.0$ meters is requested
- **THEN** target validation returns `Err(TargetOutOfRange)`.

#### Scenario: Raycast line-of-sight check
- **GIVEN** initiator entity $A$ and target entity $B$ separated by an occluding static obstacle in the octree
- **WHEN** line-of-sight validation is performed
- **THEN** targeting fails with `Err(LineOfSightBlocked)`
- **NOTE** Raycasting and cone checks operate on authoritative `Octree` Axis-Aligned Bounding Boxes (AABBs) and bounding spheres, which provide fast, deterministic volumetric visibility without requiring heavy client-side triangle meshes.

---

### Requirement: Cone / Frontal Cleave Queries

#### Scenario: Frontal arc filtering
- **GIVEN** initiator entity $A$ facing vector $(1, 0, 0)$ with cone angle $90^\circ$ and radius $8.0$ meters
- **WHEN** a cone query is executed
- **THEN** entity $C$ at $(5, 1, 0)$ is included in the hit list
- **AND** entity $D$ behind $A$ at $(-3, 0, 0)$ is excluded.

---

### Requirement: Sphere / Radial AoE Queries

#### Scenario: Radial target gathering
- **GIVEN** center coordinate $(X, Y, Z)$ and radius $R$
- **WHEN** a radial sphere query is executed
- **THEN** all entities within distance $R$ are returned up to the maximum target cap $N$
- **AND** results are sorted deterministically by ascending distance from center.
