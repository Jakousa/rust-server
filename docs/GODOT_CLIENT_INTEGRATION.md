# Godot 4 Client Integration Quickstart

This guide shows how to connect a **Godot 4** client to `mmo-gateway` using pure GDScript (`StreamPeerTCP` on port `8080` or `WebSocketPeer` on port `8443`), plus guidance for C# / GDExtension builds reusing the Rust [`mmo-schema`](../crates/schema/src/lib.rs) and [`mmo-client-wasm`](../crates/client-wasm/src/lib.rs) crates.

For the complete opcode catalog and transport options, see [WIRE_PROTOCOL_AND_CLIENT_SDK.md](./WIRE_PROTOCOL_AND_CLIENT_SDK.md).

---

## 1. Choosing a Godot 4 Transport

`mmo-gateway` defaults (configured in `crates/gateway/src/main.rs`) expose two plug-and-play transports directly supported by stock Godot 4 classes:

| Godot Class | Gateway Endpoint | Framing Notes |
|---|---|---|
| `StreamPeerTCP` | `127.0.0.1:8080` (`LISTEN_ADDR`) | Lowest overhead for desktop/mobile builds; requires accumulating bytes and looping `0x4D4D` frame decode. |
| `WebSocketPeer` | `ws://127.0.0.1:8443` (`WEBSOCKET_ADDR`, or `wss://` if `WEBSOCKET_TLS=true`) | Works for Web/HTML5 exports and restrictive firewalls. Always send **binary** frames (`put_packet`); a single incoming WS message may contain multiple coalesced `0x4D4D` frames. |

> **Coordinate space**: Engine positions use right-handed 3D floats (`Vec3(x, y, z)` in meters, matching Godot's `Vector3(x, y, z)`). Quantized yaw is `[0, 2π)` radians and pitch is `[-π/2, +π/2]` radians.

---

## 2. Reusable `MmoWire` Binary Codec (`res://net/mmo_wire.gd`)

Create `res://net/mmo_wire.gd` implementing the exact little-endian `0x4D4D` framing, LEB128 varint codecs, fixed-point dequantizers, and packet encoders/decoders matching [`crates/schema`](../crates/schema/src/client.rs).

```gdscript
class_name MmoWire
extends RefCounted

const PACKET_MAGIC: int = 0x4D4D
const HEADER_SIZE: int = 8
const MAX_PACKET_PAYLOAD_SIZE: int = 65536
const PROTOCOL_VERSION: int = 1

# Opcodes (0x0001 ..= 0x001A)
const OP_MOVEMENT: int = 0x0001
const OP_MOVEMENT_CORRECTION: int = 0x0002
const OP_INTERACTION: int = 0x0003
const OP_INTERACTION_RESULT: int = 0x0004
const OP_ENTITY_STATE: int = 0x0005
const OP_ENTITY_DESPAWN: int = 0x0006
const OP_PING: int = 0x0007
const OP_PONG: int = 0x0008
const OP_ENTITY_DELTA_BATCH: int = 0x0009
const OP_CLIENT_ACK: int = 0x000A
const OP_AUTH_REQUEST: int = 0x0011
const OP_AUTH_RESPONSE: int = 0x0012

# EntityDeltaState field_mask bits (must be decoded in ascending bit order)
const MASK_KEYFRAME: int = 1 << 0
const MASK_DELTA_POS_X: int = 1 << 1
const MASK_DELTA_POS_Y: int = 1 << 2
const MASK_DELTA_POS_Z: int = 1 << 3
const MASK_VELOCITY: int = 1 << 4
const MASK_YAW: int = 1 << 5
const MASK_PITCH: int = 1 << 6
const MASK_HEALTH: int = 1 << 7
const MASK_MAX_HEALTH: int = 1 << 8
const MASK_STATE_FLAGS: int = 1 << 9
const MASK_ACTION_ID: int = 1 << 10
const MASK_ATTRIBUTES: int = 1 << 11

# Fixed-point quantization constants (crates/schema/src/quantize.rs)
const DELTA_POS_SCALE: float = 128.0
const VELOCITY_SCALE: float = 100.0


static func frame_packet(opcode: int, payload: PackedByteArray) -> PackedByteArray:
	assert(payload.size() <= MAX_PACKET_PAYLOAD_SIZE)
	var out := PackedByteArray()
	out.resize(HEADER_SIZE + payload.size())
	out.encode_u16(0, PACKET_MAGIC)
	out.encode_u16(2, opcode)
	out.encode_u32(4, payload.size())
	for i in payload.size():
		out[HEADER_SIZE + i] = payload[i]
	return out


## Extracts complete [opcode, payload] frames from an accumulation buffer `rx_buf`,
## returning `{ "frames": Array, "error": String }`.
static func drain_frames(rx_buf: PackedByteArray) -> Dictionary:
	var frames: Array = []
	var offset: int = 0
	var total: int = rx_buf.size()

	while total - offset >= HEADER_SIZE:
		var magic: int = rx_buf.decode_u16(offset)
		if magic != PACKET_MAGIC:
			return { "frames": frames, "error": "Invalid magic 0x%04X" % magic }

		var opcode: int = rx_buf.decode_u16(offset + 2)
		var payload_len: int = rx_buf.decode_u32(offset + 4)
		if payload_len > MAX_PACKET_PAYLOAD_SIZE:
			return { "frames": frames, "error": "Payload too large: %d" % payload_len }

		var frame_size: int = HEADER_SIZE + payload_len
		if total - offset < frame_size:
			break

		var payload: PackedByteArray = rx_buf.slice(offset + HEADER_SIZE, offset + frame_size)
		frames.append({ "opcode": opcode, "payload": payload })
		offset += frame_size

	if offset > 0:
		# Compact remaining partial bytes in place
		var remaining := rx_buf.slice(offset, total)
		rx_buf.resize(remaining.size())
		for i in remaining.size():
			rx_buf[i] = remaining[i]

	return { "frames": frames, "error": "" }


## Appends an unsigned LEB128 varint (valid for unsigned 0 ..= 2^63-1 in GDScript signed int).
static func write_varint(buf: PackedByteArray, value: int) -> void:
	var v: int = value
	while true:
		var b: int = v & 0x7F
		v = (v >> 7) & 0x01FF_FFFF_FFFF_FFFF
		if v != 0:
			buf.append(b | 0x80)
		else:
			buf.append(b)
			break


## Appends a u16 LE length-prefixed UTF-8 string (used by Auth/Social/Market).
static func write_u16_str(buf: PackedByteArray, text: String) -> void:
	var utf8: PackedByteArray = text.to_utf8_buffer()
	var n: int = mini(utf8.size(), 65535)
	var hdr := PackedByteArray()
	hdr.resize(2)
	hdr.encode_u16(0, n)
	buf.append_array(hdr)
	if n > 0:
		buf.append_array(utf8.slice(0, n))


static func encode_auth_request(token: String) -> PackedByteArray:
	var payload := PackedByteArray()
	write_u16_str(payload, token)
	var ver := PackedByteArray()
	ver.resize(4)
	ver.encode_u32(0, PROTOCOL_VERSION)
	payload.append_array(ver)
	return frame_packet(OP_AUTH_REQUEST, payload)


static func encode_movement(
	entity_id: int,
	pos: Vector3,
	vel: Vector3,
	yaw: float,
	pitch: float,
	timestamp_ms: int,
	sequence: int
) -> PackedByteArray:
	var p := PackedByteArray()
	p.resize(52)
	p.encode_u64(0, entity_id)
	p.encode_float(8, pos.x)
	p.encode_float(12, pos.y)
	p.encode_float(16, pos.z)
	p.encode_float(20, vel.x)
	p.encode_float(24, vel.y)
	p.encode_float(28, vel.z)
	p.encode_float(32, yaw)
	p.encode_float(36, pitch)
	p.encode_u64(40, timestamp_ms)
	p.encode_u32(48, sequence)
	return frame_packet(OP_MOVEMENT, p)


static func encode_client_ack(session_id: int, observed_tick: int, ack_mask: int = 0) -> PackedByteArray:
	var p := PackedByteArray()
	write_varint(p, session_id)
	write_varint(p, observed_tick)
	var mask_bytes := PackedByteArray()
	mask_bytes.resize(4)
	mask_bytes.encode_u32(0, ack_mask)
	p.append_array(mask_bytes)
	return frame_packet(OP_CLIENT_ACK, p)


static func dequantize_delta_i16(q: int) -> float:
	return float(q) / DELTA_POS_SCALE


static func dequantize_velocity_i16(q: int) -> float:
	return float(q) / VELOCITY_SCALE


static func dequantize_yaw_u8(q: int) -> float:
	return (float(q) / 256.0) * TAU


static func dequantize_pitch_i8(q: int) -> float:
	return (float(clampi(q, -127, 127)) / 127.0) * (PI * 0.5)


class Cursor:
	var data: PackedByteArray
	var pos: int = 0

	func _init(bytes: PackedByteArray) -> void:
		data = bytes

	func remaining() -> int:
		return data.size() - pos

	func get_u8() -> int:
		var v: int = data.decode_u8(pos)
		pos += 1
		return v

	func get_i8() -> int:
		var v: int = data.decode_s8(pos)
		pos += 1
		return v

	func get_i16() -> int:
		var v: int = data.decode_s16(pos)
		pos += 2
		return v

	func get_u16() -> int:
		var v: int = data.decode_u16(pos)
		pos += 2
		return v

	func get_u32() -> int:
		var v: int = data.decode_u32(pos)
		pos += 4
		return v

	func get_u64() -> int:
		var v: int = data.decode_u64(pos)
		pos += 8
		return v

	func get_f32() -> float:
		var v: float = data.decode_float(pos)
		pos += 4
		return v

	func get_u16_str() -> String:
		var length: int = get_u16()
		var s: String = data.slice(pos, pos + length).get_string_from_utf8()
		pos += length
		return s

	func get_varint(max_bytes: int = 10) -> int:
		var result: int = 0
		var shift: int = 0
		for byte_idx in max_bytes:
			assert(pos < data.size(), "Truncated varint")
			var b: int = get_u8()
			result |= (b & 0x7F) << shift
			if (b & 0x80) == 0:
				return result
			shift += 7
		push_error("Varint exceeded %d bytes" % max_bytes)
		return result


static func decode_auth_response(payload: PackedByteArray) -> Dictionary:
	var c := Cursor.new(payload)
	return {
		"success": c.get_u8() != 0,
		"error_code": c.get_u32(),
		"session_id": c.get_u64(),
		"player_guid": c.get_u64(),
		"assigned_node": c.get_u16_str(),
		"error_message": c.get_u16_str(),
	}


static func decode_movement_correction(payload: PackedByteArray) -> Dictionary:
	var c := Cursor.new(payload)
	return {
		"entity_id": c.get_u64(),
		"corrected_pos": Vector3(c.get_f32(), c.get_f32(), c.get_f32()),
		"corrected_vel": Vector3(c.get_f32(), c.get_f32(), c.get_f32()),
		"sequence": c.get_u32(),
		"reason": c.get_u8(),
	}


static func decode_entity_delta_batch(payload: PackedByteArray) -> Dictionary:
	var c := Cursor.new(payload)
	var server_tick: int = c.get_varint(10)
	var baseline_tick: int = c.get_varint(10)

	var despawn_count: int = c.get_varint(5)
	var despawns: Array[int] = []
	for _i in despawn_count:
		despawns.append(c.get_varint(10))

	var delta_count: int = c.get_varint(5)
	var deltas: Array[Dictionary] = []
	for _i in delta_count:
		var entity_id: int = c.get_varint(10)
		var mask: int = c.get_u16()
		var d := { "entity_id": entity_id, "field_mask": mask }

		if (mask & MASK_KEYFRAME) != 0:
			d["position"] = Vector3(c.get_f32(), c.get_f32(), c.get_f32())
		else:
			if (mask & MASK_DELTA_POS_X) != 0:
				d["dx"] = dequantize_delta_i16(c.get_i16())
			if (mask & MASK_DELTA_POS_Y) != 0:
				d["dy"] = dequantize_delta_i16(c.get_i16())
			if (mask & MASK_DELTA_POS_Z) != 0:
				d["dz"] = dequantize_delta_i16(c.get_i16())

		if (mask & MASK_VELOCITY) != 0:
			d["velocity"] = Vector3(
				dequantize_velocity_i16(c.get_i16()),
				dequantize_velocity_i16(c.get_i16()),
				dequantize_velocity_i16(c.get_i16())
			)
		if (mask & MASK_YAW) != 0:
			d["yaw"] = dequantize_yaw_u8(c.get_u8())
		if (mask & MASK_PITCH) != 0:
			d["pitch"] = dequantize_pitch_i8(c.get_i8())
		if (mask & MASK_HEALTH) != 0:
			d["health"] = c.get_varint(5)
		if (mask & MASK_MAX_HEALTH) != 0:
			d["max_health"] = c.get_varint(5)
		if (mask & MASK_STATE_FLAGS) != 0:
			d["state_flags"] = c.get_varint(5)
		if (mask & MASK_ACTION_ID) != 0:
			d["action_id"] = c.get_varint(5)
		if (mask & MASK_ATTRIBUTES) != 0:
			var attr_count: int = c.get_varint(5)
			var attrs: Dictionary = {}
			for _a in attr_count:
				var attr_id: int = c.get_varint(5)
				var attr_val: int = c.get_varint(5)
				attrs[attr_id] = attr_val
			d["attributes"] = attrs

		deltas.append(d)

	return {
		"server_tick": server_tick,
		"baseline_tick": baseline_tick,
		"despawns": despawns,
		"deltas": deltas,
	}
```

> **Note on unsigned 64-bit values in GDScript**: GDScript's `int` is signed 64-bit, which preserves arbitrary 64-bit patterns on bit-exact `encode_u64`/`decode_u64` roundtrips. Treat `entity_id` and `session_id` as opaque identifiers (avoid signed arithmetic `<` comparisons directly on raw high-bit GUIDs).

---

## 3. Transport Node Implementation (`StreamPeerTCP` vs `WebSocketPeer`)

### Option A: Raw TCP (`StreamPeerTCP` -> `LISTEN_ADDR` `:8080`)

```gdscript
extends Node

@export var server_host: String = "127.0.0.1"
@export var server_port: int = 8080
@export var auth_token: String = "dev-local-token"

var _tcp := StreamPeerTCP.new()
var _rx_buf := PackedByteArray()
var _connected: bool = false
var _authenticated: bool = false
var _session_id: int = 0
var _player_guid: int = 0

var _seq: int = 0
var _move_accum: float = 0.0
var _pos: Vector3 = Vector3(0.0, 0.0, 5.0)
var _vel: Vector3 = Vector3.ZERO
var _entities: Dictionary = {} # entity_id -> Dictionary state
var _baseline_positions: Dictionary = {} # server_tick -> { entity_id -> Vector3 }


func _ready() -> void:
	var err := _tcp.connect_to_host(server_host, server_port)
	if err != OK:
		push_error("TCP connect failed: %s" % err)


func _process(delta: float) -> void:
	_tcp.poll()
	match _tcp.get_status():
		StreamPeerTCP.STATUS_CONNECTING:
			return
		StreamPeerTCP.STATUS_CONNECTED:
			if not _connected:
				_connected = true
				# Send AuthRequest immediately after TCP handshake
				_send_raw(MmoWire.encode_auth_request(auth_token))
		StreamPeerTCP.STATUS_ERROR, StreamPeerTCP.STATUS_NONE:
			_connected = false
			_authenticated = false
			return

	var avail := _tcp.get_available_bytes()
	if avail > 0:
		var chunk: Array = _tcp.get_data(avail)
		if chunk[0] == OK:
			_rx_buf.append_array(chunk[1])
			var drained := MmoWire.drain_frames(_rx_buf)
			if not drained["error"].is_empty():
				push_error(drained["error"])
				_tcp.disconnect_from_host()
				return
			for frame in drained["frames"]:
				_handle_frame(frame["opcode"], frame["payload"])

	if _authenticated:
		_move_accum += delta
		while _move_accum >= 0.05: # 20 Hz tick cadence
			_move_accum -= 0.05
			_send_movement_step(0.05)


func _send_raw(bytes: PackedByteArray) -> void:
	_tcp.put_data(bytes)
```

### Option B: WebSocket (`WebSocketPeer` -> `WEBSOCKET_ADDR` `:8443`)

Swap the transport loop with `WebSocketPeer`:

```gdscript
var _ws := WebSocketPeer.new()

func _ready() -> void:
	_ws.inbound_buffer_size = 262144  # Matches gateway MAX_WS_BUFFER_SIZE (256 KiB)
	_ws.outbound_buffer_size = 262144
	var err := _ws.connect_to_url("ws://127.0.0.1:8443")
	if err != OK:
		push_error("WebSocket connect failed: %s" % err)

func _process(delta: float) -> void:
	_ws.poll()
	match _ws.get_ready_state():
		WebSocketPeer.STATE_OPEN:
			if not _connected:
				_connected = true
				_send_raw(MmoWire.encode_auth_request(auth_token))
			while _ws.get_available_packet_count() > 0:
				var pkt: PackedByteArray = _ws.get_packet()
				if not _ws.was_string_packet():
					_rx_buf.append_array(pkt)
					var drained := MmoWire.drain_frames(_rx_buf)
					for frame in drained["frames"]:
						_handle_frame(frame["opcode"], frame["payload"])
		WebSocketPeer.STATE_CLOSED:
			_connected = false
			_authenticated = false
			return
	# Same 20 Hz movement loop as TCP...

func _send_raw(bytes: PackedByteArray) -> void:
	_ws.put_packet(bytes) # WebSocketPeer.put_packet sends binary frames
```

---

## 4. Handling `AuthResponse`, 20 Hz Movement, `EntityDeltaBatch`, and `ClientAck`

Attach this handler to either Option A or Option B above:

```gdscript
func _send_movement_step(dt: float) -> void:
	var input_dir := Vector3(
		Input.get_axis("ui_left", "ui_right"),
		0.0,
		Input.get_axis("ui_up", "ui_down")
	).normalized()
	_vel = input_dir * 5.0
	_pos += _vel * dt
	_seq += 1
	var now_ms := Time.get_ticks_msec()
	_send_raw(MmoWire.encode_movement(_player_guid, _pos, _vel, 0.0, 0.0, now_ms, _seq))


func _handle_frame(opcode: int, payload: PackedByteArray) -> void:
	match opcode:
		MmoWire.OP_AUTH_RESPONSE:
			var resp := MmoWire.decode_auth_response(payload)
			if resp["success"]:
				_session_id = resp["session_id"]
				_player_guid = resp["player_guid"]
				_authenticated = true
				print("Authenticated: session=%d guid=%d node=%s" % [
					_session_id, _player_guid, resp["assigned_node"]
				])
			else:
				push_error("Auth rejected (%d): %s" % [resp["error_code"], resp["error_message"]])

		MmoWire.OP_MOVEMENT_CORRECTION:
			var corr := MmoWire.decode_movement_correction(payload)
			if corr["entity_id"] == _player_guid:
				# Authoritative server correction: snap/reconcile local state
				_pos = corr["corrected_pos"]
				_vel = corr["corrected_vel"]

		MmoWire.OP_ENTITY_DELTA_BATCH:
			var batch := MmoWire.decode_entity_delta_batch(payload)
			_apply_delta_batch(batch)
			# Always acknowledge observed server_tick reliably so node advances baseline
			_send_raw(MmoWire.encode_client_ack(_session_id, batch["server_tick"], 0))


func _apply_delta_batch(batch: Dictionary) -> void:
	for guid in batch["despawns"]:
		_entities.erase(guid)

	var base_tick: int = batch["baseline_tick"]
	var server_tick: int = batch["server_tick"]
	var base_map: Dictionary = _baseline_positions.get(base_tick, {})
	var tick_snapshot: Dictionary = base_map.duplicate()

	for d in batch["deltas"]:
		var guid: int = d["entity_id"]
		if not _entities.has(guid):
			_entities[guid] = {
				"position": Vector3.ZERO,
				"velocity": Vector3.ZERO,
				"yaw": 0.0,
				"pitch": 0.0,
				"health": 0,
				"max_health": 0,
				"state_flags": 0,
				"action_id": 0,
				"attributes": {},
			}
		var ent: Dictionary = _entities[guid]
		if d.has("position"):
			ent["position"] = d["position"]
		else:
			var ref_pos: Vector3 = base_map.get(guid, ent["position"])
			ent["position"] = ref_pos + Vector3(d.get("dx", 0.0), d.get("dy", 0.0), d.get("dz", 0.0))
		tick_snapshot[guid] = ent["position"]

		for k in ["velocity", "yaw", "pitch", "health", "max_health", "state_flags", "action_id"]:
			if d.has(k):
				ent[k] = d[k]
		if d.has("attributes"):
			for attr_id in d["attributes"]:
				ent["attributes"][attr_id] = d["attributes"][attr_id]

	_baseline_positions[server_tick] = tick_snapshot
	for old_tick in _baseline_positions.keys():
		if old_tick < base_tick:
			_baseline_positions.erase(old_tick)
```

> **Note on `baseline_tick` history**: `EntityDeltaBatch` deltas (`dx`, `dy`, `dz`) are relative to the entity's position on `batch["baseline_tick"]` (within `BaselineTracker`'s `10..=120` tick window), so storing `_baseline_positions[server_tick]` keeps positions exact even when datagrams arrive out of order or frames are lost. If no baseline matches (e.g. >40 ticks / 2 s without `ClientAck`), the server falls back to sending full `MASK_KEYFRAME` positions.

---

## 5. C# and Rust GDExtension Guidance

* **Rust GDExtension (`godot-rust` / `gdext`)**: Add `mmo-schema` (and optionally `mmo-client-wasm`'s pure-Rust `PredictionBuffer`, `InterpolationBuffer`, and `ClientEntityReplica` modules) as path dependencies. Call `ClientPacket::encode` / `ClientPacket::decode_frame` directly and assert `mmo_schema::PROTOCOL_VERSION == 1` to inherit zero-copy codecs, `BitWriter`/`BitReader`, and golden-vector stability with zero manual byte parsing.
* **Godot C# (.NET 8)**: Use `System.Net.Sockets.TcpClient` or `System.Net.WebSockets.ClientWebSocket` with `System.Buffers.Binary.BinaryPrimitives` (`ReadUInt16LittleEndian`, `ReadSingleLittleEndian`, `WriteUInt64LittleEndian`), keeping `ClientAckPacket` (`0x000A`) on every processed `EntityDeltaBatchPacket` (`0x0009`).
