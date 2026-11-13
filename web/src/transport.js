import { OPCODES } from "./protocol.js";

const HEADER_SIZE = 8;
const MAGIC_0 = 0x4d;
const MAGIC_1 = 0x4d;

/**
 * Derives a fallback WebSocket URL from a WebTransport URL.
 * Uses `wss:` when the page is served over HTTPS (mixed-content compliance)
 * and `ws:` otherwise.
 * @param {string} url
 * @param {{ wsPort?: number, pageProtocol?: string }} [options]
 * @returns {string}
 */
export function deriveWebSocketUrl(url, options = {}) {
  if (url.startsWith("ws://") || url.startsWith("wss://")) {
    return url;
  }
  const wsPort = options.wsPort ?? 8443;
  const pageProto =
    options.pageProtocol ??
    (typeof window !== "undefined" ? window.location?.protocol : "http:");
  const protocol = pageProto === "https:" ? "wss:" : "ws:";
  try {
    const parsed = new URL(url);
    return `${protocol}//${parsed.hostname}:${wsPort}`;
  } catch (_) {
    return `${protocol}//127.0.0.1:${wsPort}`;
  }
}

/**
 * Network Transport Client with automatic WebTransport -> WebSocket fallback.
 *
 * Primary mode uses HTTP/3 WebTransport (UDP datagrams + bidirectional streams).
 * When WebTransport is unsupported, times out, or fails handshake (e.g. UDP blocked),
 * automatically falls back to WebSocket over TCP with identical 0x4D4D binary framing.
 */
export class WebTransportClient {
  constructor() {
    this.transport = null;
    this.ws = null;
    this.transportMode = null; // "webtransport" | "websocket" | null
    this.datagramWriter = null;
    this.streamWriter = null;
    this.connected = false;
    this.bytesReceived = 0;
    this.packetsReceived = 0;
    this.lastBytesTime = performance.now();
    this.bandwidthBps = 0;
    this.pingMs = 0;
    this.lastPingSentTime = 0;
    this.onPacketCallback = null;
    this.onStatusCallback = null;
    this._isConnecting = false;
  }

  /**
   * Sets callback for received binary packets.
   */
  onPacket(callback) {
    this.onPacketCallback = callback;
  }

  /**
   * Sets callback for connection status updates.
   */
  onStatus(callback) {
    this.onStatusCallback = callback;
  }

  /**
   * Connects to the gateway via WebTransport, automatically falling back to
   * WebSocket if WebTransport is unavailable or fails to establish.
   * @param {string} url - Target URL e.g. "https://127.0.0.1:4433" or "ws://127.0.0.1:8443"
   * @param {string} certHashHex - SHA-256 fingerprint in dotted or raw hex format.
   * @param {{ wsUrl?: string, wsPort?: number, wtTimeoutMs?: number, wsTimeoutMs?: number }} [options]
   */
  async connect(url, certHashHex = "", options = {}) {
    if (this._isConnecting) {
      return;
    }
    this._isConnecting = true;

    if (this.transport || this.ws) {
      await this.disconnect();
    }

    this.notifyStatus("connecting");

    try {
      if (url.startsWith("ws://") || url.startsWith("wss://")) {
        await this._connectWebSocket(url, options);
        return;
      }

      try {
        await this._connectWebTransport(url, certHashHex, options.wtTimeoutMs ?? 4000);
      } catch (wtErr) {
        const wsUrl = options.wsUrl || deriveWebSocketUrl(url, options);
        console.warn(
          `WebTransport failed (${wtErr?.message || wtErr}), falling back to WebSocket: ${wsUrl}`
        );
        await this._connectWebSocket(wsUrl, options);
      }
    } catch (err) {
      console.error("All transport connection attempts failed:", err);
      this.connected = false;
      this.transportMode = null;
      this.notifyStatus("disconnected", err?.message || String(err));
      throw err;
    } finally {
      this._isConnecting = false;
    }
  }

  /**
   * Attempts HTTP/3 WebTransport connection with bounded handshake timeout.
   */
  async _connectWebTransport(url, certHashHex, timeoutMs) {
    if (typeof WebTransport === "undefined") {
      throw new Error("WebTransport is not supported by this browser.");
    }

    const options = {};
    if (certHashHex && certHashHex.trim().length > 0) {
      const cleanHex = certHashHex.replace(/:/g, "").replace(/\s/g, "");
      if (cleanHex.length === 64) {
        const hashBytes = new Uint8Array(32);
        for (let i = 0; i < 32; i++) {
          hashBytes[i] = parseInt(cleanHex.substr(i * 2, 2), 16);
        }
        options.serverCertificateHashes = [
          {
            algorithm: "sha-256",
            value: hashBytes.buffer,
          },
        ];
      }
    }

    const transport = new WebTransport(url, options);
    // Prevent late unhandled promise rejections if QUIC handshake times out in background
    transport.closed.catch(() => {});

    let timer = null;
    try {
      await Promise.race([
        transport.ready,
        new Promise((_, reject) => {
          timer = setTimeout(() => {
            try {
              transport.close();
            } catch (_) {}
            reject(new Error(`WebTransport handshake timed out after ${timeoutMs}ms`));
          }, timeoutMs);
        }),
      ]);
    } finally {
      if (timer !== null) {
        clearTimeout(timer);
      }
    }

    this.transport = transport;
    this.transportMode = "webtransport";
    this.connected = true;
    this.datagramWriter = this.transport.datagrams.writable.getWriter();
    this.notifyStatus("connected");

    // Handle server-side disconnect or unexpected transport termination
    this.transport.closed
      .then(() => {
        if (this.connected && this.transportMode === "webtransport") {
          this.connected = false;
          this.transportMode = null;
          if (this.pingInterval) {
            clearInterval(this.pingInterval);
            this.pingInterval = null;
          }
          this.notifyStatus("disconnected");
        }
      })
      .catch((err) => {
        if (this.connected && this.transportMode === "webtransport") {
          this.connected = false;
          this.transportMode = null;
          if (this.pingInterval) {
            clearInterval(this.pingInterval);
            this.pingInterval = null;
          }
          this.notifyStatus("disconnected", err?.message || String(err));
        }
      });

    // Start reader loops
    this.readDatagrams();
    this.readIncomingStreams();
    this.startPingLoop();
  }

  /**
   * Connects via WebSocket (WS/WSS over TCP) with binary ArrayBuffer framing.
   */
  async _connectWebSocket(wsUrl, options = {}) {
    if (typeof WebSocket === "undefined") {
      throw new Error("WebSocket is not supported in this environment.");
    }

    const timeoutMs = options.wsTimeoutMs ?? 5000;
    const ws = new WebSocket(wsUrl);
    ws.binaryType = "arraybuffer";

    await new Promise((resolve, reject) => {
      let settled = false;
      const timer = setTimeout(() => {
        if (!settled) {
          settled = true;
          try {
            ws.close();
          } catch (_) {}
          reject(new Error(`WebSocket connection timed out after ${timeoutMs}ms`));
        }
      }, timeoutMs);

      ws.onopen = () => {
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          resolve();
        }
      };

      ws.onerror = () => {
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          reject(new Error(`WebSocket connection failed to ${wsUrl}`));
        }
      };

      ws.onclose = (ev) => {
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          reject(new Error(`WebSocket closed during handshake (code ${ev?.code ?? "unknown"})`));
        }
      };
    });

    this.ws = ws;
    this.transportMode = "websocket";
    this.connected = true;
    this.notifyStatus("connected");

    ws.onmessage = (event) => {
      if (!this.connected) return;
      let bytes;
      if (event.data instanceof ArrayBuffer) {
        bytes = new Uint8Array(event.data);
      } else if (ArrayBuffer.isView(event.data)) {
        bytes = new Uint8Array(event.data.buffer, event.data.byteOffset, event.data.byteLength);
      } else {
        return;
      }

      this.trackIngress(bytes.byteLength);
      this._processWebSocketBinary(bytes);
    };

    ws.onclose = (ev) => {
      if (this.connected && this.transportMode === "websocket") {
        this.connected = false;
        this.transportMode = null;
        if (this.pingInterval) {
          clearInterval(this.pingInterval);
          this.pingInterval = null;
        }
        const reason = ev?.reason || (ev?.code && ev.code !== 1000 ? `code ${ev.code}` : null);
        this.notifyStatus("disconnected", reason);
      }
    };

    ws.onerror = () => {
      if (this.connected && this.transportMode === "websocket") {
        console.warn("WebSocket transport error");
      }
    };

    this.startPingLoop();
  }

  /**
   * Extracts one or more 0x4D4D-framed binary packets from a WebSocket message.
   */
  _processWebSocketBinary(bytes) {
    let offset = 0;
    while (offset + HEADER_SIZE <= bytes.byteLength) {
      if (bytes[offset] !== MAGIC_0 || bytes[offset + 1] !== MAGIC_1) {
        console.warn("Invalid 0x4D4D magic header in WebSocket frame");
        break;
      }
      const view = new DataView(bytes.buffer, bytes.byteOffset + offset, HEADER_SIZE);
      const payloadLen = view.getUint32(4, true);
      const totalLen = HEADER_SIZE + payloadLen;
      if (offset + totalLen > bytes.byteLength) {
        console.warn("Truncated 0x4D4D frame in WebSocket message");
        break;
      }
      const frameSlice = bytes.subarray(offset, offset + totalLen);
      try {
        this.handleIncomingPacket(frameSlice);
      } catch (packetErr) {
        console.warn("Error processing WebSocket packet:", packetErr);
      }
      offset += totalLen;
    }
  }

  /**
   * Internal processor for incoming binary frames, computing ping RTT on PONG frames.
   */
  handleIncomingPacket(value) {
    if (value && value.byteLength >= 8) {
      const view = new DataView(value.buffer, value.byteOffset, value.byteLength);
      if (view.getUint8(0) === MAGIC_0 && view.getUint8(1) === MAGIC_1) {
        const opcode = view.getUint16(2, true);
        if (opcode === OPCODES.PONG && value.byteLength >= 24) {
          const clientTimestamp = Number(view.getBigUint64(8, true));
          const now = performance.now();
          this.pingMs = Math.max(0, now - clientTimestamp);
          return;
        }
      }
    }
    if (this.onPacketCallback) {
      this.onPacketCallback(value);
    }
  }

  /**
   * Asynchronously reads incoming datagrams (ephemeral stream).
   */
  async readDatagrams() {
    const reader = this.transport.datagrams.readable.getReader();
    try {
      while (this.connected) {
        const { value, done } = await reader.read();
        if (done) break;
        if (value) {
          this.trackIngress(value.byteLength);
          try {
            this.handleIncomingPacket(value);
          } catch (packetErr) {
            console.warn("Error processing datagram packet:", packetErr);
          }
        }
      }
    } catch (err) {
      if (this.connected) {
        console.warn("Datagram reader terminated:", err);
      }
    } finally {
      reader.releaseLock();
    }
  }

  /**
   * Asynchronously accepts incoming bidirectional streams.
   */
  async readIncomingStreams() {
    const streamReader = this.transport.incomingBidirectionalStreams.getReader();
    try {
      while (this.connected) {
        const { value: biStream, done } = await streamReader.read();
        if (done) break;
        if (biStream) {
          this.handleIncomingStream(biStream.readable);
        }
      }
    } catch (err) {
      if (this.connected) {
        console.warn("Incoming stream reader terminated:", err);
      }
    } finally {
      streamReader.releaseLock();
    }
  }

  /**
   * Streams data from a single incoming bidirectional stream.
   */
  async handleIncomingStream(readableStream) {
    const reader = readableStream.getReader();
    try {
      while (this.connected) {
        const { value, done } = await reader.read();
        if (done) break;
        if (value) {
          this.trackIngress(value.byteLength);
          this.handleIncomingPacket(value);
        }
      }
    } catch (err) {
      console.warn("Stream read error:", err);
    } finally {
      reader.releaseLock();
    }
  }

  /**
   * Sends an unreliable datagram to the gateway (or WebSocket binary frame in fallback mode).
   */
  async sendDatagram(data) {
    if (!this.connected) return;
    if (this.transportMode === "websocket") {
      if (this.ws && this.ws.readyState === 1) {
        try {
          this.ws.send(data);
        } catch (err) {
          console.warn("Failed to send WebSocket datagram:", err);
        }
      }
      return;
    }
    if (!this.datagramWriter) return;
    try {
      await this.datagramWriter.write(data);
    } catch (err) {
      console.warn("Failed to send datagram:", err);
    }
  }

  /**
   * Sends reliable bytes via a bidirectional stream (or WebSocket binary frame in fallback mode).
   */
  async sendStream(data) {
    if (!this.connected) return;
    if (this.transportMode === "websocket") {
      if (this.ws && this.ws.readyState === 1) {
        try {
          this.ws.send(data);
        } catch (err) {
          console.warn("Failed to send WebSocket stream data:", err);
        }
      }
      return;
    }
    if (!this.transport) return;
    try {
      if (!this.streamWriter) {
        const biStream = await this.transport.createBidirectionalStream();
        this.streamWriter = biStream.writable.getWriter();
        this.handleIncomingStream(biStream.readable);
      }
      await this.streamWriter.write(data);
    } catch (err) {
      console.warn("Failed to send stream data:", err);
      this.streamWriter = null;
    }
  }

  /**
   * Periodically measures round-trip time.
   */
  startPingLoop() {
    this.pingInterval = setInterval(() => {
      if (!this.connected) return;
      this.lastPingSentTime = performance.now();
      // Client packet ping: Magic [0x4D, 0x4D], Opcode 0x0007 (OPCODES.PING), Payload timestamp (8 bytes)
      const pingBuf = new Uint8Array(16);
      pingBuf[0] = MAGIC_0;
      pingBuf[1] = MAGIC_1;
      const view = new DataView(pingBuf.buffer);
      view.setUint16(2, OPCODES.PING, true);
      view.setUint32(4, 8, true); // Payload len 8
      view.setBigUint64(8, BigInt(Math.floor(this.lastPingSentTime)), true);
      this.sendDatagram(pingBuf);
    }, 2000);
  }

  /**
   * Records ingress byte counts to calculate bandwidth.
   */
  trackIngress(bytes) {
    this.bytesReceived += bytes;
    this.packetsReceived += 1;
    const now = performance.now();
    const dt = (now - this.lastBytesTime) / 1000;
    if (dt >= 1.0) {
      this.bandwidthBps = this.bytesReceived / dt / 1024; // KB/s
      this.bytesReceived = 0;
      this.lastBytesTime = now;
    }
  }

  notifyStatus(status, error = null) {
    if (this.onStatusCallback) {
      this.onStatusCallback({ status, error, mode: this.transportMode });
    }
  }

  async disconnect() {
    this.connected = false;
    this.transportMode = null;
    if (this.pingInterval) {
      clearInterval(this.pingInterval);
      this.pingInterval = null;
    }
    if (this.datagramWriter) {
      try {
        await this.datagramWriter.close();
      } catch (_) {}
      this.datagramWriter = null;
    }
    this.streamWriter = null;
    if (this.transport) {
      try {
        await this.transport.close();
      } catch (_) {}
      this.transport = null;
    }
    if (this.ws) {
      try {
        this.ws.onopen = null;
        this.ws.onmessage = null;
        this.ws.onclose = null;
        this.ws.onerror = null;
        this.ws.close();
      } catch (_) {}
      this.ws = null;
    }
    this.notifyStatus("disconnected");
  }
}
