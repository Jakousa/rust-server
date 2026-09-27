/**
 * HUD Telemetry & User Interface Controller.
 */
export class UIController {
  constructor() {
    this.statusDot = document.getElementById("status-dot");
    this.gatewayUrlInput = document.getElementById("gateway-url");
    this.certHashInput = document.getElementById("cert-hash");
    this.btnConnect = document.getElementById("btn-connect");
    this.btnDisconnect = document.getElementById("btn-disconnect");

    this.valPing = document.getElementById("val-ping");
    this.valTick = document.getElementById("val-tick");
    this.valBaseline = document.getElementById("val-baseline");
    this.valEntities = document.getElementById("val-entities");
    this.valPackets = document.getElementById("val-packets");
    this.valBandwidth = document.getElementById("val-bandwidth");
    this.valPos = document.getElementById("val-pos");
    this.fpsCounter = document.getElementById("fps-counter");

    this.togglePartitions = document.getElementById("toggle-partitions");
    this.toggleAoi = document.getElementById("toggle-aoi");
    this.toggleWireframe = document.getElementById("toggle-wireframe");

    this.frameCount = 0;
    this.lastFpsUpdate = performance.now();
  }

  initEvents({ onConnect, onDisconnect, onTogglePartitions, onToggleAoi, onToggleWireframe }) {
    this.btnConnect.addEventListener("click", () => {
      const url = this.gatewayUrlInput.value;
      const certHash = this.certHashInput.value;
      if (onConnect) onConnect(url, certHash);
    });

    this.btnDisconnect.addEventListener("click", () => {
      if (onDisconnect) onDisconnect();
    });

    this.togglePartitions.addEventListener("change", (e) => {
      if (onTogglePartitions) onTogglePartitions(e.target.checked);
    });

    this.toggleAoi.addEventListener("change", (e) => {
      if (onToggleAoi) onToggleAoi(e.target.checked);
    });

    this.toggleWireframe.addEventListener("change", (e) => {
      if (onToggleWireframe) onToggleWireframe(e.target.checked);
    });
  }

  setConnectionState(state) {
    this.statusDot.className = `status-indicator ${state}`;
    if (state === "connected") {
      this.btnConnect.disabled = true;
      this.btnDisconnect.disabled = false;
    } else if (state === "disconnected") {
      this.btnConnect.disabled = false;
      this.btnDisconnect.disabled = true;
    } else if (state === "connecting") {
      this.btnConnect.disabled = true;
      this.btnDisconnect.disabled = false;
    }
  }

  updateTelemetry(telemetryJson, pingMs, bandwidthKb, pos) {
    let tele = {};
    try {
      if (typeof telemetryJson === "string") {
        tele = JSON.parse(telemetryJson);
      } else {
        tele = telemetryJson || {};
      }
    } catch (_) {}

    if (this.valTick && tele.latest_server_tick !== undefined) {
      this.valTick.textContent = tele.latest_server_tick;
    }
    if (this.valBaseline && tele.latest_baseline_tick !== undefined) {
      this.valBaseline.textContent = tele.latest_baseline_tick;
    }
    if (this.valEntities && tele.entity_count !== undefined) {
      this.valEntities.textContent = tele.entity_count;
    }
    if (this.valPackets && tele.received_packets !== undefined) {
      this.valPackets.textContent = tele.received_packets;
    }
    if (this.valPing) {
      this.valPing.textContent = pingMs > 0 ? `${Math.round(pingMs)} ms` : "-- ms";
    }
    if (this.valBandwidth) {
      this.valBandwidth.textContent = `${bandwidthKb.toFixed(1)} KB/s`;
    }
    if (this.valPos && pos) {
      this.valPos.textContent = `X: ${pos.x.toFixed(1)}, Y: ${pos.y.toFixed(1)}, Z: ${pos.z.toFixed(1)}`;
    }

    // FPS counter calculation
    this.frameCount++;
    const now = performance.now();
    if (now - this.lastFpsUpdate >= 1000) {
      const fps = Math.round((this.frameCount * 1000) / (now - this.lastFpsUpdate));
      this.fpsCounter.textContent = `${fps} FPS`;
      this.frameCount = 0;
      this.lastFpsUpdate = now;
    }
  }
}
