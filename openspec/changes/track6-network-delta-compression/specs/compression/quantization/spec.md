# Specification: Fixed-Point Kinematic Quantization

## Capability: `compression/quantization`

### Context
Standard 32-bit floating point representations (`f32`) allocate 4 bytes per spatial coordinate and rotation angle (12 bytes for 3D position, 12 bytes for velocity, 8 bytes for yaw/pitch). For networked game state synchronization, millimeter-level sub-atomic precision is redundant and wasteful. Fixed-point quantization maps continuous floating-point ranges to compact discrete integers with bounded error margins.

---

### Scenario: Delta Coordinate Quantization
GIVEN a 3D position displacement $(\Delta x, \Delta y, \Delta z)$ relative to a baseline state where each $|\Delta| \le 256.0\text{ meters}$
WHEN quantized using signed 16-bit fixed-point at $1/128\text{m}$ resolution (`scale = 128.0`)
THEN each coordinate axis is stored in 2 bytes (total 6 bytes for 3D vector instead of 12 bytes)
AND the maximum quantization reconstruction error $|val_{reconstructed} - val_{original}|$ must not exceed $1/256\text{m} \approx 0.0039\text{m}$ ($3.9\text{mm}$).

### Scenario: World Coordinate Keyframe Quantization
GIVEN a full world position $(x, y, z)$ within world bounds $[-1000.0, 1000.0]$
WHEN quantized for a keyframe update
THEN 21-bit or 24-bit fixed-point quantization provides sub-millimeter precision across a $2\text{km}$ world
AND reconstruction recovers coordinates within $0.001\text{m}$ of original floats.

### Scenario: Yaw and Pitch Angular Quantization
GIVEN an entity yaw angle in $[-\pi, \pi]$ radians ($[-180^\circ, 180^\circ]$) and pitch angle in $[-\pi/2, \pi/2]$
WHEN quantized into 8-bit unsigned integer (`u8`) for yaw and 8-bit signed integer (`i8`) for pitch
THEN yaw consumes exactly 1 byte with angular resolution $\approx 360^\circ / 256 \approx 1.41^\circ$
AND pitch consumes exactly 1 byte with angular resolution $\approx 180^\circ / 128 \approx 1.41^\circ$
AND total rotation consumes 2 bytes instead of 8 bytes (75% savings).

### Scenario: Velocity Quantization
GIVEN a 3D velocity vector with speed up to $50.0\text{ m/s}$
WHEN quantized into signed 16-bit integers with scale factor $100.0$ ($1\text{cm/s}$ resolution)
THEN velocity consumes 6 bytes on the wire
AND the velocity reconstruction error does not exceed $0.005\text{ m/s}$.

### Scenario: Out-of-Range Clamping & Overflow Guard
GIVEN a catastrophic kinematic delta exceeding the representable range $[-256.0, 256.0]$
WHEN quantization is executed
THEN the values are saturated to the min/max representable integer bounds without arithmetic overflow or wrapping
AND the server flags a keyframe requirement for that entity.
