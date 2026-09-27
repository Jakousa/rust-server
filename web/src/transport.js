/**
 * WebTransport Network Client.
 *
 * Multiplexes high-frequency unreliable datagrams (movement, entity deltas)
 * and reliable bidirectional streams (interactions, chat, auth), with
 * support for SHA-256 certificate hash pinning for development self-signed certs.
 */
export class WebTransportClient {
  constructor() {
    this.transport = null;
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
   * Connects to WebTransport endpoint.
   * @param {string} url - Target URL e.g. "https://127.0.0.1:4433"
   * @param {string} certHashHex - SHA-256 fingerprint in dotted or raw hex format.
   */
  async connect(url, certHashHex = "") {
    if (this.transport) {
      await this.disconnect();
    }

    this.notifyStatus("connecting");

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

    try {
      if (typeof WebTransport === "undefined") {
        throw new Error("WebTransport is not supported by this browser.");
      }

      this.transport = new WebTransport(url, options);
      await this.transport.ready;

      this.connected = true;
      this.datagramWriter = this.transport.datagrams.writable.getWriter();
      this.notifyStatus("connected");

      // Start reader loops
      this.readDatagrams();
      this.readIncomingStreams();
      this.startPingLoop();
    } catch (err) {
      console.error("WebTransport connection failed:", err);
      this.connected = false;
      this.notifyStatus("disconnected", err.message);
      throw err;
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
          if (this.onPacketCallback) {
            this.onPacketCallback(value);
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
          if (this.onPacketCallback) {
            this.onPacketCallback(value);
          }
        }
      }
    } catch (err) {
      console.warn("Stream read error:", err);
    } finally {
      reader.releaseLock();
    }
  }

  /**
   * Sends an unreliable datagram to the gateway.
   */
  async sendDatagram(data) {
    if (!this.connected || !this.datagramWriter) return;
    try {
      await this.datagramWriter.write(data);
    } catch (err) {
      console.warn("Failed to send datagram:", err);
    }
  }

  /**
   * Sends reliable bytes via a bidirectional stream.
   */
  async sendStream(data) {
    if (!this.connected || !this.transport) return;
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
      // Client packet ping: Magic [0x4D, 0x4D], Opcode 0x0006, Payload timestamp (8 bytes)
      const pingBuf = new Uint8Array(16);
      pingBuf[0] = 0x4D;
      pingBuf[1] = 0x4D;
      pingBuf[2] = 0x06; // Opcode Ping
      pingBuf[3] = 0x00;
      pingBuf[4] = 0x08; // Length 8
      pingBuf[5] = 0x00;
      pingBuf[6] = 0x00;
      pingBuf[7] = 0x00;
      const view = new DataView(pingBuf.buffer);
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
      this.bandwidthBps = (this.bytesReceived / dt) / 1024; // KB/s
      this.bytesReceived = 0;
      this.lastBytesTime = now;
    }
  }

  notifyStatus(status, error = null) {
    if (this.onStatusCallback) {
      this.onStatusCallback({ status, error });
    }
  }

  async disconnect() {
    this.connected = false;
    if (this.pingInterval) {
      clearInterval(this.pingInterval);
    }
    if (this.datagramWriter) {
      try {
        await this.datagramWriter.close();
      } catch (_) {}
      this.datagramWriter = null;
    }
    if (this.transport) {
      try {
        await this.transport.close();
      } catch (_) {}
      this.transport = null;
    }
    this.notifyStatus("disconnected");
  }
}
