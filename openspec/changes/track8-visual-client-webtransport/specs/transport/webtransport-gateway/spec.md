# Specification: WebTransport Gateway Ingress

## Overview
Extends `mmo-gateway` with a WebTransport (HTTP/3 over QUIC/UDP) endpoint operating in parallel with the existing TCP listener. Provides low-latency, multiplexed unreliable datagram and reliable stream delivery for browser-based clients without WebRTC complexity.

---

## 1. Transport Architecture

```
Browser Client (Chrome/Firefox/Safari)
      │
      │  UDP / QUIC (Port 4433)
      ▼
┌────────────────────────────────────────────────────────┐
│ mmo-gateway                                            │
│                                                        │
│  ┌────────────────────────┐  ┌──────────────────────┐  │
│  │ WebTransport Listener  │  │     TCP Listener     │  │
│  │ (HTTP/3 / QUIC)        │  │   (Port 8080)        │  │
│  └───────────┬────────────┘  └──────────┬───────────┘  │
│              │                          │              │
│              ▼                          ▼              │
│       ┌────────────────────────────────────────┐       │
│       │             SessionRouter              │       │
│       └───────────────────┬────────────────────┘       │
└───────────────────────────┼────────────────────────────┘
                            │
                            ▼ gRPC / Channels
                  Authoritative Node (20 Hz)
```

---

## 2. Protocol & Channel Mapping

| Packet Opcode | Name | Delivery Mode | Channel | Rationale |
|---|---|---|---|---|
| `0x0001` | `MovementPacket` | Unreliable | Datagram | High frequency (20 Hz). Dropped frames are superseded by newer inputs. |
| `0x0009` | `EntityDeltaBatchPacket` | Unreliable | Datagram | High frequency delta compression stream. Baseline ACKs handle loss recovery. |
| `0x000A` | `ClientAckPacket` | Unreliable | Datagram | Cumulative bitmask ACKs. Next ACK carries updated state. |
| `0x0005` | `InteractionPacket` | Reliable | Bi-directional Stream | Actions, cast requests, and trades must not be silently dropped. |
| `0x0006` | `InteractionResultPacket`| Reliable | Bi-directional Stream | Authoritative feedback for cast actions and target confirmation. |
| `0x0007`/`0x0008`| `PingPacket` / `PongPacket` | Unreliable | Datagram | RTT and jitter measurement. |

---

## 3. Cryptographic Security & Development Certificates
1. **TLS 1.3 Mandate**:
   - WebTransport strictly enforces TLS 1.3 over QUIC.
2. **Self-Signed Ephemeral Certificate Support**:
   - For local development and testing, `mmo-gateway` generates an ephemeral ECDSA certificate upon startup using `rcgen` / `wtransport`.
   - The gateway outputs the SHA-256 fingerprint hash (32 bytes hex/base64).
   - Browser clients specify `serverCertificateHashes: [{ algorithm: "sha-256", value: Uint8Array }]` during `new WebTransport(...)`, eliminating the need to install root CAs or deal with browser trust errors.

---

## 4. Session Ingress & Routing Invariants
1. **Uniform Session ID**:
   - Each WebTransport connection is assigned an authoritative `session_id: u64` identical to TCP sessions.
2. **Datagram Demuxing**:
   - When a datagram is received from session $S$:
     1. Verify length $\ge 8$ bytes and frame magic `0x4D4D`.
     2. Read 16-bit opcode.
     3. Route `ClientPacket` to the assigned simulation node address via `SessionRouter`.
3. **Egress Demuxing**:
   - When the simulation node sends an AoI broadcast batch for session $S$:
     - If packet is `EntityDeltaBatchPacket`, send via `session.send_datagram(payload)`.
     - If payload exceeds MTU ($1,200\text{ bytes}$), clamp or chunk into sub-batches.
