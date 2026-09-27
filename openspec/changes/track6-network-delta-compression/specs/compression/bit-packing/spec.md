# Specification: Bit-Packed Field Masks & Variable-Length Encoding

## Capability: `compression/bit-packing`

### Context
Replicating multiple entities each tick requires transmitting only fields that experienced modifications. In uncompressed replication, even untouched fields (e.g. constant health, stationary axes) consume 32 or 64 bits each. This capability introduces compact field bitmasks and variable-length integer encoding.

---

### Scenario: Variable-Length Integer Encoding & Decoding
GIVEN a 64-bit unsigned integer value `N`
WHEN encoded into a buffer using LEB128 varint format
THEN values $< 128$ must occupy exactly 1 byte
AND values $< 16384$ must occupy exactly 2 bytes
AND decoding the buffer must reconstruct the original value `N` without truncation or data corruption.

### Scenario: Bitmask Selective Serialization
GIVEN an entity state with updated fields (e.g., `position_x`, `position_z`, `yaw`) and untouched fields (`position_y`, `velocity`, `health`, `state_flags`)
WHEN serialized using `DeltaFieldMask`
THEN only modified fields corresponding to set bits in the mask are appended to the payload
AND the field mask itself consumes at most 2 bytes
AND deserialization with the mask updates only the specified fields, leaving other cached entity fields unchanged.

### Scenario: BitWriter and BitReader Bit-Level Packing
GIVEN a sequence of arbitrary bit-width fields (e.g. 3-bit flags, 12-bit signed offsets, 8-bit quantized angles)
WHEN written using `BitWriter`
THEN bits are packed sequentially into consecutive bytes with zero padding between sub-byte fields
AND byte alignment can be asserted or flushed when switching to byte-aligned payloads
AND `BitReader` reading the bitstream extracts each value with exact bitwise fidelity.

### Scenario: Malformed Buffer Bounds Protection
GIVEN an incoming packet buffer truncated or corrupted mid-bitstream
WHEN `BitReader` or varint decoder attempts to read beyond the buffer's slice bounds
THEN a `ClientCodecError::BufferTooShort` is returned immediately without panics, memory out-of-bounds, or infinite loops.
