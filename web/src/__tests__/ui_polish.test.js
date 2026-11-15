import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { UIController } from "../ui.js";
import { PlayerControls } from "../controls.js";
import { WebTransportClient } from "../transport.js";

function createMockElement(id = "", className = "") {
  const el = {
    id,
    className,
    style: {},
    children: [],
    textContent: "",
    title: "",
    value: "",
    disabled: false,
    _innerHTML: "",
    get innerHTML() {
      return this._innerHTML;
    },
    set innerHTML(val) {
      this._innerHTML = val;
    },
  };
  el.classList = {
    add(...tokens) {
      const current = new Set((el.className || "").split(/\s+/).filter(Boolean));
      tokens.forEach((t) => current.add(t));
      el.className = Array.from(current).join(" ");
    },
    remove(...tokens) {
      const current = new Set((el.className || "").split(/\s+/).filter(Boolean));
      tokens.forEach((t) => current.delete(t));
      el.className = Array.from(current).join(" ");
    },
    contains(token) {
      return (el.className || "").split(/\s+/).filter(Boolean).includes(token);
    },
  };
  return el;
}

describe("UIController Connection Badges & Toast Polish", () => {
  let elements;

  beforeEach(() => {
    elements = new Map();
    elements.set("status-dot", createMockElement("status-dot", "status-indicator disconnected"));
    elements.set("status-text", createMockElement("status-text", "status-badge disconnected"));
    elements.set("transport-badge", createMockElement("transport-badge", "transport-badge offline"));
    elements.set("hud-toast", createMockElement("hud-toast", "hud-toast hidden"));
    elements.set("connection-error", createMockElement("connection-error", "connection-error hidden"));
    elements.set("btn-connect", createMockElement("btn-connect"));
    elements.set("btn-disconnect", createMockElement("btn-disconnect"));

    globalThis.document = {
      getElementById(id) {
        return elements.has(id) ? elements.get(id) : null;
      },
      createElement() {
        return createMockElement();
      },
    };
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("updates #status-text and #transport-badge across connection states and transportMode values", () => {
    const ui = new UIController();
    const statusText = document.getElementById("status-text");
    const transportBadge = document.getElementById("transport-badge");

    ui.setConnectionState("connecting", "Connecting...", "");
    expect(statusText.textContent).toBe("Connecting...");
    expect(statusText.className).toBe("status-badge connecting");
    expect(transportBadge.textContent).toBe("Negotiating");
    expect(transportBadge.className).toBe("transport-badge connecting");

    ui.setConnectionState("connected", "Connected", "", "webtransport");
    expect(statusText.textContent).toBe("Connected");
    expect(statusText.className).toBe("status-badge connected");
    expect(transportBadge.textContent).toBe("HTTP/3");
    expect(transportBadge.className).toBe("transport-badge webtransport");

    ui.setConnectionState("connected", "Connected", "", "websocket");
    expect(transportBadge.textContent).toBe("WebSocket");
    expect(transportBadge.className).toBe("transport-badge websocket");

    ui.setConnectionState("disconnected", "Disconnected", "Lost peer", "");
    expect(statusText.textContent).toBe("Disconnected");
    expect(statusText.className).toBe("status-badge disconnected");
    expect(transportBadge.textContent).toBe("Offline");
    expect(transportBadge.className).toBe("transport-badge offline");
  });

  it("remains 100% backward-compatible with 3-argument setConnectionState callers", () => {
    const ui = new UIController();
    const statusText = document.getElementById("status-text");
    const transportBadge = document.getElementById("transport-badge");
    const errorEl = document.getElementById("connection-error");

    ui.setConnectionState("connected", "Connected", "");
    expect(statusText.textContent).toBe("Connected");
    expect(transportBadge.textContent).toBe("Online");
    expect(transportBadge.className).toBe("transport-badge online");

    // Defaults text cleanly when omitted
    ui.setConnectionState("connecting");
    expect(statusText.textContent).toBe("Connecting...");
    expect(transportBadge.textContent).toBe("Negotiating");

    ui.setConnectionState("disconnected", "Disconnected", "Connection timed out");
    expect(statusText.textContent).toBe("Disconnected");
    expect(transportBadge.textContent).toBe("Offline");
    expect(errorEl.classList.contains("hidden")).toBe(false);
    expect(errorEl.textContent).toBe("Connection timed out");

    // Preserves remembered transport mode if 3-arg connected follows a 4-arg connected call
    ui.setConnectionState("connected", "Connected", "", "webtransport");
    ui.setConnectionState("connected", "Connected", "");
    expect(transportBadge.textContent).toBe("HTTP/3");
  });

  it("displays timed toast notifications with variant classes and resets timer on repeated calls", () => {
    vi.useFakeTimers();
    const ui = new UIController();
    const toast = document.getElementById("hud-toast");

    ui.showToast("Barter successful: +1 Iron Pickaxe", "success", 2500);
    expect(toast.textContent).toBe("Barter successful: +1 Iron Pickaxe");
    expect(toast.classList.contains("hidden")).toBe(false);
    expect(toast.classList.contains("toast-success")).toBe(true);
    expect(toast._innerHTML).toBe("");

    // Advance 2000ms (still visible), then replace with another toast
    vi.advanceTimersByTime(2000);
    expect(toast.classList.contains("hidden")).toBe(false);

    ui.showToast("Switched to WebSocket fallback", "warning", 3000);
    expect(toast.textContent).toBe("Switched to WebSocket fallback");
    expect(toast.classList.contains("toast-warning")).toBe(true);
    expect(toast.classList.contains("toast-success")).toBe(false);

    // Past original 2500ms window (2000 + 1000 = 3000ms): new toast must still be visible
    vi.advanceTimersByTime(1000);
    expect(toast.classList.contains("hidden")).toBe(false);

    // Remaining 2000ms elapses -> auto-dismisses
    vi.advanceTimersByTime(2000);
    expect(toast.classList.contains("hidden")).toBe(true);
  });

  it("handles missing #status-text, #transport-badge, and #hud-toast DOM nodes gracefully", () => {
    globalThis.document = {
      getElementById() {
        return null;
      },
    };

    const ui = new UIController();
    expect(() => ui.setConnectionState("connected", "Connected", "", "webtransport")).not.toThrow();
    expect(() => ui.showToast("Hello", "info", 1000)).not.toThrow();
    expect(() => ui.hideToast()).not.toThrow();
  });
});

describe("WebTransportClient notifyStatus transportMode", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("includes transportMode alongside mode in notifyStatus callbacks", () => {
    const client = new WebTransportClient();
    const events = [];
    client.onStatus((payload) => events.push(payload));

    client.transportMode = "webtransport";
    client.notifyStatus("connected");

    client.transportMode = "websocket";
    client.notifyStatus("disconnected", "Network lost");

    expect(events).toEqual([
      { status: "connected", error: null, mode: "webtransport", transportMode: "webtransport" },
      { status: "disconnected", error: "Network lost", mode: "websocket", transportMode: "websocket" },
    ]);
  });

  it("emits transportMode during WebTransport connect and WebSocket fallback", async () => {
    const client = new WebTransportClient();
    const events = [];
    client.onStatus((payload) => events.push(payload));

    vi.stubGlobal(
      "WebTransport",
      vi.fn().mockImplementation(() => ({
        ready: Promise.reject(new Error("UDP blocked")),
        closed: Promise.resolve(),
        close: vi.fn(),
      }))
    );

    class MockWebSocket {
      constructor(url) {
        this.url = url;
        this.binaryType = "";
        this.readyState = 1;
        this.send = vi.fn();
        this.close = vi.fn();
        queueMicrotask(() => {
          if (this.onopen) this.onopen();
        });
      }
    }
    vi.stubGlobal("WebSocket", MockWebSocket);

    await client.connect("https://127.0.0.1:4433", "", { wsPort: 8443 });
    expect(events.some((e) => e.status === "connected" && e.transportMode === "websocket")).toBe(
      true
    );

    await client.disconnect();
    expect(events[events.length - 1]).toEqual({
      status: "disconnected",
      error: null,
      mode: null,
      transportMode: null,
    });
  });
});

describe("PlayerControls Editable Input Isolation", () => {
  it("ignores keydowns targeting INPUT, TEXTAREA, SELECT, or contentEditable elements", () => {
    const controls = new PlayerControls();
    const castSpy = vi.fn();
    controls.onCast(castSpy);

    const editableTargets = [
      { tagName: "INPUT" },
      { tagName: "input" },
      { tagName: "TEXTAREA" },
      { tagName: "SELECT" },
      { tagName: "DIV", isContentEditable: true },
    ];

    for (const target of editableTargets) {
      controls.setKeyState({ key: "w", code: "KeyW", target }, true);
      controls.setKeyState({ key: "ArrowUp", code: "ArrowUp", target }, true);
      controls.setKeyState({ key: " ", code: "Space", target }, true);
    }

    expect(controls.keys.w).toBe(false);
    expect(controls.keys.arrowUp).toBe(false);
    expect(controls.keys.space).toBe(false);
    expect(castSpy).not.toHaveBeenCalled();

    // Keyup still clears held movement keys even if an input gained focus
    controls.setKeyState({ key: "w", code: "KeyW", target: { tagName: "CANVAS" } }, true);
    expect(controls.keys.w).toBe(true);
    controls.setKeyState({ key: "w", code: "KeyW", target: { tagName: "INPUT" } }, false);
    expect(controls.keys.w).toBe(false);

    // Space outside editable inputs triggers cast normally
    controls.setKeyState({ key: " ", code: "Space", target: { tagName: "CANVAS" } }, true);
    expect(castSpy).toHaveBeenCalledTimes(1);
  });
});
