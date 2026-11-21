import { test, expect } from "@playwright/test";

const HUD_PANEL_SELECTORS = [
  "#game-mode-switcher",
  ".connection-panel",
  ".telemetry-panel",
  ".debug-panel",
  "#controls-hint",
  "#village-hud .inventory-panel",
  "#target-panel",
  "#asteroid-hud .ship-status-panel",
  "#asteroid-hud .cargo-vault-panel",
  "#asteroid-target-panel",
];

function isExpectedOfflineGatewayConsoleError(msg) {
  const text = msg.text() || "";
  const locUrl = msg.location()?.url || "";
  const combined = `${locUrl} ${text}`;
  if (process.env.E2E_BASE_URL) {
    return (
      /:(4433|4434)\b/.test(combined) &&
      /ERR_TUNNEL_CONNECTION_FAILED|ERR_QUIC_PROTOCOL_ERROR|WebTransport/i.test(combined)
    );
  }
  return (
    /:(4433|4434|8443|8444|9091|9092)\b/.test(combined) ||
    /\/(village|asteroid)\/ws\b/.test(combined) ||
    /ERR_QUIC_PROTOCOL_ERROR|All transport connection attempts failed/i.test(text)
  );
}

function attachErrorCollectors(page) {
  const pageErrors = [];
  const rawConsoleErrors = [];
  const unexpectedConsoleErrors = [];

  page.on("pageerror", (err) => pageErrors.push(err.message));
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      const entry = `${msg.text()}${msg.location()?.url ? ` (${msg.location().url})` : ""}`;
      rawConsoleErrors.push(entry);
      if (!isExpectedOfflineGatewayConsoleError(msg)) {
        unexpectedConsoleErrors.push(entry);
      }
    }
  });

  return { pageErrors, rawConsoleErrors, unexpectedConsoleErrors };
}

async function assertWebGLCanvasReady(page) {
  const canvas = page.locator("canvas");
  await expect(canvas).toHaveCount(1);
  const canvasBox = await canvas.boundingBox();
  expect(canvasBox).not.toBeNull();
  expect(canvasBox.width).toBeGreaterThanOrEqual(1200);
  expect(canvasBox.height).toBeGreaterThanOrEqual(700);

  const hasWebGL = await page.evaluate(() => {
    const c = document.querySelector("canvas");
    return !!(c && c.width > 0 && c.height > 0 && (c.getContext("webgl2") || c.getContext("webgl")));
  });
  expect(hasWebGL).toBe(true);
}

async function assertNoOverlappingHudPanels(page, expectedMode) {
  const layout = await page.evaluate((selectors) => {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const docEl = document.documentElement;
    const visiblePanels = [];

    for (const sel of selectors) {
      const el = document.querySelector(sel);
      if (!el) continue;
      const style = window.getComputedStyle(el);
      const r = el.getBoundingClientRect();
      if (
        style.display === "none" ||
        style.visibility === "hidden" ||
        r.width <= 0 ||
        r.height <= 0
      ) {
        continue;
      }
      visiblePanels.push({
        selector: sel,
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
        width: r.width,
        height: r.height,
      });
    }

    const overlaps = [];
    for (let i = 0; i < visiblePanels.length; i++) {
      for (let j = i + 1; j < visiblePanels.length; j++) {
        const a = visiblePanels[i];
        const b = visiblePanels[j];
        const intersects =
          a.right > b.left + 0.5 &&
          b.right > a.left + 0.5 &&
          a.bottom > b.top + 0.5 &&
          b.bottom > a.top + 0.5;
        if (intersects) {
          overlaps.push({ a, b });
        }
      }
    }

    const outOfBounds = visiblePanels.filter(
      (p) => p.left < -0.5 || p.top < -0.5 || p.right > vw + 0.5 || p.bottom > vh + 0.5
    );

    return {
      vw,
      vh,
      scrollWidth: docEl.scrollWidth,
      scrollHeight: docEl.scrollHeight,
      visibleSelectors: visiblePanels.map((p) => p.selector),
      overlaps,
      outOfBounds,
      gameModeAttr: document.body?.dataset?.gameMode || "",
    };
  }, HUD_PANEL_SELECTORS);

  expect(layout.scrollWidth).toBeLessThanOrEqual(layout.vw);
  expect(layout.scrollHeight).toBeLessThanOrEqual(layout.vh);
  expect(layout.outOfBounds).toEqual([]);
  expect(layout.overlaps).toEqual([]);
  if (expectedMode) {
    expect(layout.gameModeAttr).toBe(expectedMode);
  }
}

async function assertModalFitsViewport(page, modalSelector, windowSelector) {
  const metrics = await page.evaluate(
    ({ mSel, wSel }) => {
      const modal = document.querySelector(mSel);
      const win = document.querySelector(wSel) || modal?.firstElementChild;
      if (!modal || !win) return null;
      const r = win.getBoundingClientRect();
      return {
        display: window.getComputedStyle(modal).display,
        vw: window.innerWidth,
        vh: window.innerHeight,
        scrollWidth: document.documentElement.scrollWidth,
        scrollHeight: document.documentElement.scrollHeight,
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
        width: r.width,
        height: r.height,
      };
    },
    { mSel: modalSelector, wSel: windowSelector }
  );

  expect(metrics).not.toBeNull();
  expect(metrics.display).not.toBe("none");
  expect(metrics.width).toBeGreaterThan(200);
  expect(metrics.height).toBeGreaterThan(120);
  expect(metrics.left).toBeGreaterThanOrEqual(-0.5);
  expect(metrics.top).toBeGreaterThanOrEqual(-0.5);
  expect(metrics.right).toBeLessThanOrEqual(metrics.vw + 0.5);
  expect(metrics.bottom).toBeLessThanOrEqual(metrics.vh + 0.5);
  expect(metrics.scrollWidth).toBeLessThanOrEqual(metrics.vw);
  expect(metrics.scrollHeight).toBeLessThanOrEqual(metrics.vh);
}

test.describe("OpenSpec MMO Web Inspector E2E", () => {
  test("boots WebGL scene and renders non-overlapping HUD panels without page errors", async ({
    page,
  }) => {
    const { pageErrors, unexpectedConsoleErrors } = attachErrorCollectors(page);

    await page.goto("/", { waitUntil: "domcontentloaded" });
    await expect(page).toHaveTitle(/OpenSpec MMO/);
    await page.waitForTimeout(300);

    await assertWebGLCanvasReady(page);

    // Core HUD panels should be visible
    await expect(page.locator(".connection-panel")).toBeVisible();
    await expect(page.locator(".telemetry-panel")).toBeVisible();
    await expect(page.locator(".debug-panel")).toBeVisible();
    await expect(page.locator(".controls-hint")).toBeVisible();
    await expect(page.locator(".inventory-panel")).toBeVisible();

    // Target panel and modals should start hidden
    await expect(page.locator("#target-panel")).toBeHidden();
    await expect(page.locator("#npc-barter-modal")).toBeHidden();
    await expect(page.locator("#player-trade-modal")).toBeHidden();

    await assertNoOverlappingHudPanels(page, "village");

    expect(pageErrors).toEqual([]);
    expect(unexpectedConsoleErrors).toEqual([]);
  });

  test("resolves gateway URL and handles debug toggles", async ({ page }) => {
    await page.goto("/", { waitUntil: "domcontentloaded" });

    const gatewayUrl = await page.inputValue("#gateway-url");
    expect(gatewayUrl).toMatch(/^https:\/\/.+:4433$/);

    // Toggle visual debug checkboxes
    const partitions = page.locator("#toggle-partitions");
    const aoi = page.locator("#toggle-aoi");
    const wireframe = page.locator("#toggle-wireframe");

    await expect(partitions).toBeChecked();
    await expect(aoi).toBeChecked();
    await expect(wireframe).not.toBeChecked();

    await partitions.uncheck();
    await aoi.uncheck();
    await wireframe.check();

    await expect(partitions).not.toBeChecked();
    await expect(aoi).not.toBeChecked();
    await expect(wireframe).toBeChecked();
  });

  test("updates local player position on WASD movement and supports mouse orbit", async ({
    page,
  }) => {
    await page.goto("/", { waitUntil: "domcontentloaded" });

    const posEl = page.locator("#val-pos");
    const initialPos = await posEl.textContent();
    expect(initialPos).toContain("X: 0.0, Y: 0.0, Z:");

    // Move forward with W
    await page.keyboard.down("w");
    await page.waitForTimeout(350);
    await page.keyboard.up("w");

    const afterW = await posEl.textContent();
    expect(afterW).not.toEqual(initialPos);

    // Mouse drag orbit on canvas should not throw
    const { pageErrors } = attachErrorCollectors(page);
    await page.mouse.move(640, 400);
    await page.mouse.down();
    await page.mouse.move(740, 360, { steps: 5 });
    await page.mouse.up();
    await page.mouse.wheel(0, 120);

    expect(pageErrors).toEqual([]);
  });

  test("auto-discovers /cert-hash when served behind gateway ingress", async ({
    page,
  }) => {
    const isLive = !!process.env.E2E_BASE_URL;
    test.skip(!isLive, "Live ingress /cert-hash check runs when E2E_BASE_URL is set");

    await page.goto("/", { waitUntil: "networkidle" });
    const certHash = await page.inputValue("#cert-hash");
    expect(certHash).toMatch(/^[0-9a-f]{2}(:[0-9a-f]{2}){31}$/i);
  });

  test("boots Village RPG on /village and Asteroid Miners on /asteroid with non-overlapping HUDs and clean logs", async ({
    page,
  }) => {
    const { pageErrors, unexpectedConsoleErrors } = attachErrorCollectors(page);

    await page.goto("/asteroid", { waitUntil: "domcontentloaded" });
    await page.waitForTimeout(300);
    await assertWebGLCanvasReady(page);
    await expect(page.locator("#asteroid-hud")).toBeVisible();
    await expect(page.locator("#village-hud")).toBeHidden();
    await expect(page.locator("#btn-mode-asteroid")).toHaveClass(/active/);
    await expect(page.locator("#btn-mode-village")).not.toHaveClass(/active/);
    await expect(page.locator("#controls-hint-title")).toContainText("6DoF");
    expect(await page.inputValue("#gateway-url")).toMatch(/:4434$/);
    await assertNoOverlappingHudPanels(page, "asteroid");

    await page.goto("/village", { waitUntil: "domcontentloaded" });
    await page.waitForTimeout(300);
    await assertWebGLCanvasReady(page);
    await expect(page.locator("#village-hud")).toBeVisible();
    await expect(page.locator("#asteroid-hud")).toBeHidden();
    await expect(page.locator("#btn-mode-village")).toHaveClass(/active/);
    await expect(page.locator("#btn-mode-asteroid")).not.toHaveClass(/active/);
    await expect(page.locator("#controls-hint-title")).toContainText("Village");
    expect(await page.inputValue("#gateway-url")).toMatch(/:4433$/);
    await assertNoOverlappingHudPanels(page, "village");

    expect(pageErrors).toEqual([]);
    expect(unexpectedConsoleErrors).toEqual([]);
  });

  test("dynamically switches HUDs, URL pathname, and gateway port via mode switcher buttons", async ({
    page,
  }) => {
    const { pageErrors, unexpectedConsoleErrors } = attachErrorCollectors(page);

    await page.goto("/village", { waitUntil: "domcontentloaded" });
    await expect(page.locator("#village-hud")).toBeVisible();
    expect(new URL(page.url()).pathname).toBe("/village");
    expect(await page.inputValue("#gateway-url")).toMatch(/:4433$/);

    // Switch Village -> Asteroid
    await page.click("#btn-mode-asteroid");
    await expect(page.locator("#asteroid-hud")).toBeVisible();
    await expect(page.locator("#village-hud")).toBeHidden();
    await expect(page.locator("#btn-mode-asteroid")).toHaveClass(/active/);
    await expect(page.locator("#btn-mode-village")).not.toHaveClass(/active/);
    expect(new URL(page.url()).pathname).toBe("/asteroid");
    expect(await page.inputValue("#gateway-url")).toMatch(/:4434$/);
    await assertWebGLCanvasReady(page);
    await assertNoOverlappingHudPanels(page, "asteroid");

    // Switch Asteroid -> Village
    await page.click("#btn-mode-village");
    await expect(page.locator("#village-hud")).toBeVisible();
    await expect(page.locator("#asteroid-hud")).toBeHidden();
    await expect(page.locator("#btn-mode-village")).toHaveClass(/active/);
    await expect(page.locator("#btn-mode-asteroid")).not.toHaveClass(/active/);
    expect(new URL(page.url()).pathname).toBe("/village");
    expect(await page.inputValue("#gateway-url")).toMatch(/:4433$/);
    await assertWebGLCanvasReady(page);
    await assertNoOverlappingHudPanels(page, "village");

    expect(pageErrors).toEqual([]);
    expect(unexpectedConsoleErrors).toEqual([]);
  });

  test("opens and closes NPC Barter and Player Trade modals cleanly within 1280x800 viewport", async ({
    page,
  }) => {
    const { pageErrors, unexpectedConsoleErrors } = attachErrorCollectors(page);

    await page.goto("/village", { waitUntil: "domcontentloaded" });
    await expect(page.locator("#npc-barter-modal")).toBeHidden();
    await expect(page.locator("#player-trade-modal")).toBeHidden();

    // Open NPC Barter modal with realistic recipe rows via safe DOM APIs
    await page.evaluate(() => {
      const modal = document.getElementById("npc-barter-modal");
      const list = document.getElementById("barter-recipes-list");
      list.replaceChildren();
      for (let i = 1; i <= 4; i++) {
        const card = document.createElement("div");
        card.className = "barter-recipe-card";
        const exchange = document.createElement("div");
        exchange.className = "recipe-exchange";
        const reqs = document.createElement("span");
        reqs.className = "recipe-reqs";
        reqs.textContent = `${i}x Iron Ore + ${i * 5} Gold`;
        const arrow = document.createElement("span");
        arrow.className = "barter-arrow";
        arrow.textContent = "->";
        const rewards = document.createElement("span");
        rewards.className = "recipe-rewards";
        rewards.textContent = `1x Crafted Tool #${i}`;
        exchange.append(reqs, arrow, rewards);
        const btn = document.createElement("button");
        btn.className = "barter-action-btn";
        btn.textContent = "Barter";
        card.append(exchange, btn);
        list.appendChild(card);
      }
      modal.style.display = "flex";
    });

    await expect(page.locator("#npc-barter-modal")).toBeVisible();
    await assertModalFitsViewport(page, "#npc-barter-modal", ".barter-window");

    await page.click("#btn-close-barter");
    await expect(page.locator("#npc-barter-modal")).toBeHidden();

    // Open Player Trade modal with sample offer items
    await page.evaluate(() => {
      const modal = document.getElementById("player-trade-modal");
      const sessionEl = document.getElementById("trade-session-id");
      const myItems = document.getElementById("trade-my-items");
      const partnerItems = document.getElementById("trade-partner-items");
      sessionEl.textContent = "901";

      myItems.replaceChildren();
      for (const name of ["Iron Ingot", "Oak Plank", "Amber Gem"]) {
        const row = document.createElement("div");
        row.className = "trade-item-chip";
        const lbl = document.createElement("span");
        lbl.textContent = `2x ${name}`;
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "trade-item-toggle";
        btn.textContent = "+ Offer";
        row.append(lbl, btn);
        myItems.appendChild(row);
      }

      partnerItems.replaceChildren();
      const pRow = document.createElement("div");
      pRow.className = "trade-item-chip";
      pRow.textContent = "1x Runed Pickaxe";
      partnerItems.appendChild(pRow);

      modal.style.display = "flex";
    });

    await expect(page.locator("#player-trade-modal")).toBeVisible();
    await expect(page.locator("#trade-offer-gold")).toBeVisible();
    await expect(page.locator("#btn-trade-offer")).toBeVisible();
    await expect(page.locator("#btn-trade-accept")).toBeVisible();
    await expect(page.locator("#btn-trade-cancel")).toBeVisible();
    await assertModalFitsViewport(page, "#player-trade-modal", ".trade-window");

    await page.click("#btn-trade-cancel");
    await expect(page.locator("#player-trade-modal")).toBeHidden();

    // Re-open and close via header close button
    await page.evaluate(() => {
      document.getElementById("player-trade-modal").style.display = "flex";
    });
    await expect(page.locator("#player-trade-modal")).toBeVisible();
    await page.click("#btn-close-trade");
    await expect(page.locator("#player-trade-modal")).toBeHidden();

    expect(pageErrors).toEqual([]);
    expect(unexpectedConsoleErrors).toEqual([]);
  });

  test("exercises live Village NPC Barter & Player Trade packet lifecycle via simulated gateway with zero console errors", async ({
    page,
  }) => {
    test.skip(
      !!process.env.E2E_BASE_URL,
      "Simulated WebSocket gateway harness runs only when E2E_BASE_URL is unset"
    );

    const { pageErrors, rawConsoleErrors } = attachErrorCollectors(page);

    await page.route("**/cert-hash", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          cert_hash:
            "aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99",
        }),
      })
    );

    await page.addInitScript(() => {
      // Disable WebTransport so WebTransportClient deterministically connects over MockWebSocket
      try {
        delete window.WebTransport;
      } catch (_) {
        window.WebTransport = undefined;
      }

      const OPCODE_AUTH_RESPONSE = 0x0012;
      const OPCODE_INTERACTION = 0x0003;
      const OPCODE_INTERACTION_RESULT = 0x0004;
      const OPCODE_ENTITY_DELTA_BATCH = 0x0009;
      const OPCODE_AUTH_REQUEST = 0x0011;

      function writeVarint(val, out) {
        let v = BigInt(val);
        while (v >= 0x80n) {
          out.push(Number((v & 0x7fn) | 0x80n));
          v >>= 7n;
        }
        out.push(Number(v & 0x7fn));
      }

      function makeFrame(opcode, payloadBytes) {
        const payload = payloadBytes instanceof Uint8Array ? payloadBytes : new Uint8Array(payloadBytes);
        const frame = new Uint8Array(8 + payload.length);
        const view = new DataView(frame.buffer);
        frame[0] = 0x4d;
        frame[1] = 0x4d;
        view.setUint16(2, opcode, true);
        view.setUint32(4, payload.length, true);
        frame.set(payload, 8);
        return frame;
      }

      function makeInteractionResult(actionId, jsonObj) {
        const jsonBytes = new TextEncoder().encode(JSON.stringify(jsonObj));
        const payload = new Uint8Array(13 + jsonBytes.length);
        const view = new DataView(payload.buffer);
        view.setUint32(0, actionId >>> 0, true);
        payload[4] = 1;
        view.setUint32(5, 0, true);
        view.setUint32(9, jsonBytes.length, true);
        payload.set(jsonBytes, 13);
        return makeFrame(OPCODE_INTERACTION_RESULT, payload);
      }

      function makeNpcSpawnBatch() {
        const p = [];
        writeVarint(1, p); // server_tick
        writeVarint(0, p); // baseline_tick
        p.push(0); // 0 despawns
        p.push(1); // 1 delta
        const npcGuid = (2n << 48n) | 101n; // Villager NPC (type=2)
        writeVarint(npcGuid, p);
        const mask = 0x0001; // MASK_KEYFRAME
        p.push(mask & 0xff, (mask >> 8) & 0xff);
        const f = new ArrayBuffer(12);
        const fv = new DataView(f);
        fv.setFloat32(0, 0.0, true);
        fv.setFloat32(4, 0.0, true);
        fv.setFloat32(8, 3.5, true); // 1.5m from player spawn (0, 0, 5)
        new Uint8Array(f).forEach((b) => p.push(b));
        return makeFrame(OPCODE_ENTITY_DELTA_BATCH, p);
      }

      let activeSocket = null;
      window.__mockGatewayPushTradeSession = () => {
        if (!activeSocket || !activeSocket.onmessage) return;
        const pkt = makeInteractionResult((0x0002 << 16) | 4, {
          active_trade: {
            session_id: 505,
            my_offer: { currency: 0, items: [] },
            their_offer: {
              currency: 30,
              items: [{ item_id: 901, name: "Moonstone", quantity: 1 }],
            },
            my_locked: false,
            their_locked: false,
            my_accepted: false,
            their_accepted: false,
            completed: false,
          },
        });
        activeSocket.onmessage({ data: pkt.buffer });
      };

      class MockWebSocket {
        constructor(url) {
          this.url = url;
          this.binaryType = "arraybuffer";
          this.readyState = 0;
          activeSocket = this;
          setTimeout(() => {
            this.readyState = 1;
            if (this.onopen) this.onopen();
          }, 10);
        }

        send(data) {
          const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
          if (bytes.byteLength < 8 || bytes[0] !== 0x4d || bytes[1] !== 0x4d) return;
          const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
          const opcode = view.getUint16(2, true);

          if (opcode === OPCODE_AUTH_REQUEST) {
            const authPayload = new Uint8Array(25);
            const av = new DataView(authPayload.buffer);
            authPayload[0] = 1;
            av.setUint32(1, 0, true);
            av.setBigUint64(5, 42n, true);
            av.setBigUint64(13, (1n << 48n) | 1n, true);
            setTimeout(() => {
              this.onmessage?.({ data: makeFrame(OPCODE_AUTH_RESPONSE, authPayload).buffer });
              this.onmessage?.({ data: makeNpcSpawnBatch().buffer });
            }, 10);
            return;
          }

          if (opcode === OPCODE_INTERACTION && bytes.byteLength >= 33) {
            const targetGuid = view.getBigUint64(16, true);
            const actionId = view.getUint32(25, true);
            const subOp = actionId & 0xffff;
            const inventory = {
              currency: 50,
              current_weight: 6.0,
              max_weight: 100.0,
              slots: [
                { item_id: 101, name: "Iron Ore", quantity: 3, unit_weight: 2.0 },
                { item_id: 102, name: "Herb Bundle", quantity: 1, unit_weight: 0.5 },
              ],
            };

            if (subOp === 1 && targetGuid === 0n) {
              setTimeout(() => {
                this.onmessage?.({ data: makeInteractionResult(actionId, { inventory }).buffer });
              }, 10);
            } else if (subOp === 1) {
              setTimeout(() => {
                this.onmessage?.({
                  data: makeInteractionResult(actionId, {
                    inventory,
                    target_merchant: {
                      name: "Blacksmith Ilmari",
                      role: "Village Smith",
                      greeting: "Fine steel for honest barter.",
                      recipes: [
                        {
                          recipe_id: 7,
                          required_items: [{ name: "Iron Ore", quantity: 2 }],
                          required_currency: 10,
                          reward_items: [{ name: "Iron Pickaxe", quantity: 1 }],
                          reward_currency: 35,
                        },
                      ],
                    },
                  }).buffer,
                });
              }, 10);
            } else if (subOp === 2) {
              setTimeout(() => {
                this.onmessage?.({
                  data: makeInteractionResult(actionId, {
                    inventory: {
                      currency: 75,
                      current_weight: 4.0,
                      max_weight: 100.0,
                      slots: [{ item_id: 103, name: "Iron Pickaxe", quantity: 1, unit_weight: 4.0 }],
                    },
                  }).buffer,
                });
              }, 10);
            } else if (subOp === 4) {
              setTimeout(() => {
                this.onmessage?.({
                  data: makeInteractionResult(actionId, {
                    active_trade: {
                      session_id: 505,
                      my_offer: { currency: 15, items: [] },
                      their_offer: {
                        currency: 30,
                        items: [{ item_id: 901, name: "Moonstone", quantity: 1 }],
                      },
                      my_locked: true,
                      their_locked: true,
                      my_accepted: false,
                      their_accepted: false,
                      completed: false,
                    },
                  }).buffer,
                });
              }, 10);
            }
          }
        }

        close() {
          this.readyState = 3;
        }
      }

      window.WebSocket = MockWebSocket;
    });

    await page.goto("/village", { waitUntil: "domcontentloaded" });

    await expect(page.locator("#status-text")).toHaveText("Connected");
    await expect(page.locator("#transport-badge")).toHaveText("WebSocket");
    await expect(page.locator("#val-gold")).toHaveText("50");
    await expect(page.locator("#target-panel")).toBeVisible();
    await assertNoOverlappingHudPanels(page, "village");

    // Open NPC Barter modal via Interact button
    await page.click("#btn-interact");
    await expect(page.locator("#npc-barter-modal")).toBeVisible();
    await assertModalFitsViewport(page, "#npc-barter-modal", ".barter-window");

    await page.click('.barter-action-btn[data-recipe-id="7"]');
    await expect(page.locator("#val-gold")).toHaveText("75");
    await page.click("#btn-close-barter");
    await expect(page.locator("#npc-barter-modal")).toBeHidden();

    // Push incoming player trade session and exercise offer + cancel flow
    await page.evaluate(() => window.__mockGatewayPushTradeSession());
    await expect(page.locator("#player-trade-modal")).toBeVisible();
    await assertModalFitsViewport(page, "#player-trade-modal", ".trade-window");

    await page.locator("#trade-my-items .trade-item-toggle").first().click();
    await page.fill("#trade-offer-gold", "15");
    await page.click("#btn-trade-offer");
    await expect(page.locator("#trade-status-msg")).toHaveText("Both locked");

    await page.click("#btn-trade-cancel");
    await expect(page.locator("#player-trade-modal")).toBeHidden();

    expect(pageErrors).toEqual([]);
    expect(rawConsoleErrors).toEqual([]);
  });

  test("streams VXR1 asteroid chunks after auth, renders asteroid belt, mines ore, engages NPC drone, banks loot, and exercises Playtest Workbench on /asteroid", async ({
    page,
  }) => {
    test.skip(
      !!process.env.E2E_BASE_URL,
      "Simulated WebSocket gateway harness runs only when E2E_BASE_URL is unset"
    );

    const { pageErrors, unexpectedConsoleErrors } = attachErrorCollectors(page);

    await page.route("**/asteroid/cert-hash", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          cert_hash:
            "aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99",
        }),
      })
    );

    await page.addInitScript(() => {
      try {
        delete window.WebTransport;
      } catch (_) {
        window.WebTransport = undefined;
      }

      const OPCODE_MOVEMENT = 0x0001;
      const OPCODE_INTERACTION = 0x0003;
      const OPCODE_INTERACTION_RESULT = 0x0004;
      const OPCODE_ENTITY_DELTA_BATCH = 0x0009;
      const OPCODE_AUTH_REQUEST = 0x0011;
      const OPCODE_AUTH_RESPONSE = 0x0012;

      const VOXEL_MODULE_ID = 0x00a0;
      const ASTEROID_MODULE_ID = 0x0003;

      const SEEDED_KEYS = new Set([
        "0,0,-2",
        "1,0,-1",
        "-2,0,0",
        "0,1,-1",
        "1,0,1",
        "-1,-1,-2",
        "-1,0,-2",
        "0,-1,-2",
        "-1,0,1",
        "1,-1,-1",
      ]);

      window.__mockAsteroidStats = {
        receivedPreAuthPacket: false,
        authCompleted: false,
        chunkRequests: [],
        modifyRequests: 0,
        pvpShots: 0,
        bankCalls: 0,
      };

      function writeVarint(val, out) {
        let v = BigInt(val);
        while (v >= 0x80n) {
          out.push(Number((v & 0x7fn) | 0x80n));
          v >>= 7n;
        }
        out.push(Number(v & 0x7fn));
      }

      function makeFrame(opcode, payloadBytes) {
        const payload = payloadBytes instanceof Uint8Array ? payloadBytes : new Uint8Array(payloadBytes);
        const frame = new Uint8Array(8 + payload.length);
        const view = new DataView(frame.buffer);
        frame[0] = 0x4d;
        frame[1] = 0x4d;
        view.setUint16(2, opcode, true);
        view.setUint32(4, payload.length, true);
        frame.set(payload, 8);
        return frame;
      }

      function makeInteractionResultBytes(actionId, payloadBytes) {
        const payload = new Uint8Array(13 + payloadBytes.length);
        const view = new DataView(payload.buffer);
        view.setUint32(0, actionId >>> 0, true);
        payload[4] = 1;
        view.setUint32(5, 0, true);
        view.setUint32(9, payloadBytes.length, true);
        payload.set(payloadBytes, 13);
        return makeFrame(OPCODE_INTERACTION_RESULT, payload);
      }

      function makeInteractionResultJson(actionId, jsonObj) {
        return makeInteractionResultBytes(
          actionId,
          new TextEncoder().encode(JSON.stringify(jsonObj))
        );
      }

      function encodeVxr1ChunkPacket(cx, cy, cz, revision = 0n) {
        const voxels = new Uint16Array(4096);
        const seedSalt = Math.abs(cx) * 7 + Math.abs(cy) * 13 + Math.abs(cz) * 19;
        for (let lz = 1; lz < 15; lz++) {
          for (let ly = 1; ly < 15; ly++) {
            for (let lx = 1; lx < 15; lx++) {
              const dist = Math.hypot(lx - 7.5, ly - 7.5, lz - 7.5);
              const bump = ((lx * 31 + ly * 17 + lz * 13 + seedSalt) % 5) * 0.2;
              let v = 0;
              if (dist <= 1.8) v = 4;
              else if (dist <= 3.1) v = 3;
              else if (dist <= 4.5) v = 2;
              else if (dist <= 5.6 + bump) v = (lx * 3 + ly * 5 + lz * 7 + seedSalt) % 11 === 0 ? 3 : 1;
              voxels[lx + ly * 16 + lz * 256] = v;
            }
          }
        }
        const spans = [];
        let cur = voxels[0];
        let count = 1;
        for (let i = 1; i < 4096; i++) {
          if (voxels[i] === cur) {
            count++;
          } else {
            spans.push([count, cur]);
            cur = voxels[i];
            count = 1;
          }
        }
        spans.push([count, cur]);

        const out = new Uint8Array(12 + 16 + spans.length * 4);
        const view = new DataView(out.buffer);
        view.setInt32(0, cx, true);
        view.setInt32(4, cy, true);
        view.setInt32(8, cz, true);
        out[12] = 0x56;
        out[13] = 0x58;
        out[14] = 0x52;
        out[15] = 0x31;
        view.setBigUint64(16, revision, true);
        view.setUint32(24, spans.length, true);
        let off = 28;
        for (const [c, v] of spans) {
          view.setUint16(off, c, true);
          view.setUint16(off + 2, v, true);
          off += 4;
        }
        return out;
      }

      let droneHp = 100;
      let tickCounter = 1;
      function makeDroneBatch(hpVal) {
        const p = [];
        writeVarint(tickCounter++, p);
        writeVarint(0, p);
        p.push(0); // 0 despawns
        p.push(1); // 1 delta
        const droneGuid = (2n << 48n) | 1n; // EntityType::Npc
        writeVarint(droneGuid, p);
        const mask = 0x0001 | 0x0080 | 0x0100 | 0x0200; // KEYFRAME | HEALTH | MAX_HEALTH | STATE_FLAGS
        p.push(mask & 0xff, (mask >> 8) & 0xff);
        const f = new ArrayBuffer(12);
        const fv = new DataView(f);
        fv.setFloat32(0, 20.0, true);
        fv.setFloat32(4, 0.0, true);
        fv.setFloat32(8, -18.0, true);
        new Uint8Array(f).forEach((b) => p.push(b));
        writeVarint(hpVal, p);
        writeVarint(100, p);
        writeVarint(10 << 16, p);
        return makeFrame(OPCODE_ENTITY_DELTA_BATCH, p);
      }

      const simState = {
        player_guid: Number((1n << 48n) | 1n),
        hull_hp: 100,
        max_hull_hp: 100,
        shininess_tier: 0,
        unbanked_cargo_value: 0,
        current_weight: 0,
        max_weight: 150,
        in_safe_zone: true,
        inventory: { currency: 0, max_weight: 150, slots: [] },
        station_vault: { banked_credits: 0, banked_items: [], kills: 0, deaths: 0 },
      };

      class MockWebSocket {
        constructor(url) {
          this.url = url;
          this.binaryType = "arraybuffer";
          this.readyState = 0;
          setTimeout(() => {
            this.readyState = 1;
            if (this.onopen) this.onopen();
          }, 10);
        }

        send(data) {
          const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
          if (bytes.byteLength < 8 || bytes[0] !== 0x4d || bytes[1] !== 0x4d) return;
          const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
          const opcode = view.getUint16(2, true);

          if (opcode !== OPCODE_AUTH_REQUEST && !window.__mockAsteroidStats.authCompleted) {
            window.__mockAsteroidStats.receivedPreAuthPacket = true;
          }

          if (opcode === OPCODE_AUTH_REQUEST) {
            const authPayload = new Uint8Array(25);
            const av = new DataView(authPayload.buffer);
            authPayload[0] = 1;
            av.setUint32(1, 0, true);
            av.setBigUint64(5, 77n, true);
            av.setBigUint64(13, (1n << 48n) | 1n, true);
            setTimeout(() => {
              window.__mockAsteroidStats.authCompleted = true;
              this.onmessage?.({ data: makeFrame(OPCODE_AUTH_RESPONSE, authPayload).buffer });
              this.onmessage?.({ data: makeDroneBatch(droneHp).buffer });
            }, 15);
            return;
          }

          if (opcode === OPCODE_MOVEMENT) {
            return;
          }

          if (opcode === OPCODE_INTERACTION && bytes.byteLength >= 33) {
            const actionId = view.getUint32(25, true);
            const moduleId = (actionId >>> 16) & 0xffff;
            const subOp = actionId & 0xffff;
            const payloadLen = view.getUint32(29, true);
            const payload = bytes.subarray(33, 33 + payloadLen);

            if (moduleId === VOXEL_MODULE_ID && subOp === 1 && payload.byteLength >= 12) {
              const pv = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
              const cx = pv.getInt32(0, true);
              const cy = pv.getInt32(4, true);
              const cz = pv.getInt32(8, true);
              const key = `${cx},${cy},${cz}`;
              window.__mockAsteroidStats.chunkRequests.push(key);
              if (SEEDED_KEYS.has(key)) {
                const chunkData = encodeVxr1ChunkPacket(cx, cy, cz, 0n);
                setTimeout(() => {
                  this.onmessage?.({ data: makeInteractionResultBytes(actionId, chunkData).buffer });
                }, 5);
              }
              return;
            }

            if (moduleId === VOXEL_MODULE_ID && subOp === 4 && payload.byteLength >= 26) {
              window.__mockAsteroidStats.modifyRequests += 1;
              const pv = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
              const vx = pv.getBigInt64(0, true);
              const vy = pv.getBigInt64(8, true);
              const vz = pv.getBigInt64(16, true);
              const newVoxel = pv.getUint16(24, true);

              simState.inventory.slots.push({
                item_id: 2002,
                name: "Solar Gold",
                quantity: 1,
                unit_weight: 3.0,
              });
              simState.current_weight += 3.0;
              simState.unbanked_cargo_value += 30;
              simState.shininess_tier = Math.min(255, Math.floor(simState.unbanked_cargo_value / 5));

              const resp = new Uint8Array(27);
              const rv = new DataView(resp.buffer);
              resp[0] = 0;
              rv.setBigInt64(1, vx, true);
              rv.setBigInt64(9, vy, true);
              rv.setBigInt64(17, vz, true);
              rv.setUint16(25, newVoxel, true);
              setTimeout(() => {
                this.onmessage?.({ data: makeInteractionResultBytes(actionId, resp).buffer });
              }, 5);
              return;
            }

            if (moduleId === ASTEROID_MODULE_ID) {
              if (subOp === 1) {
                setTimeout(() => {
                  this.onmessage?.({ data: makeInteractionResultJson(actionId, simState).buffer });
                }, 5);
              } else if (subOp === 2) {
                window.__mockAsteroidStats.pvpShots += 1;
                droneHp = Math.max(0, droneHp - 25);
                setTimeout(() => {
                  this.onmessage?.({ data: makeDroneBatch(droneHp).buffer });
                  this.onmessage?.({ data: makeInteractionResultJson(actionId, simState).buffer });
                }, 5);
              } else if (subOp === 3) {
                window.__mockAsteroidStats.bankCalls += 1;
                simState.station_vault.banked_credits += simState.unbanked_cargo_value;
                for (const s of simState.inventory.slots) {
                  simState.station_vault.banked_items.push({ ...s });
                }
                simState.inventory.slots = [];
                simState.current_weight = 0;
                simState.unbanked_cargo_value = 0;
                simState.shininess_tier = 0;
                setTimeout(() => {
                  this.onmessage?.({ data: makeInteractionResultJson(actionId, simState).buffer });
                }, 5);
              }
            }
          }
        }

        close() {
          this.readyState = 3;
        }
      }

      window.WebSocket = MockWebSocket;
    });

    await page.goto("/asteroid", { waitUntil: "domcontentloaded" });

    await expect(page.locator("#status-text")).toHaveText("Connected");
    await expect(page.locator("#transport-badge")).toHaveText("WebSocket");
    await expect(page.locator("#asteroid-target-panel")).toBeVisible();
    await expect(page.locator("#asteroid-target-name")).toContainText("Rogue Drone #");
    await expect(page.locator("#btn-trade-pilot")).toBeHidden();

    // Verify ALL visible panels including #asteroid-target-panel fit cleanly in 1280x800 without overlap
    await assertNoOverlappingHudPanels(page, "asteroid");

    // Verify NO pre-auth packets leaked and authoritative chunks (revision=0) replaced preview chunks
    await page.click("#btn-toggle-devtools");
    await expect(page.locator("#devtools-drawer")).toBeVisible();
    await expect(page.locator("#devtools-stats")).toContainText("Chunks: 10 (10 srv / 0 prv)");
    await page.click("#btn-close-devtools");
    await expect(page.locator("#devtools-drawer")).toBeHidden();

    const mockStats = await page.evaluate(() => window.__mockAsteroidStats);
    expect(mockStats.receivedPreAuthPacket).toBe(false);
    expect(mockStats.chunkRequests.length).toBeGreaterThanOrEqual(10);

    // Engage Flight Assist to fly near the closest asteroid (exiting safe zone), then Mine Ore
    await page.click("#btn-fly-asteroid");
    await expect(page.locator("#asteroid-safe-zone-badge")).toHaveText("PVP OPEN SPACE", {
      timeout: 6000,
    });
    await page.click("#btn-brake-ship");
    await page.click("#btn-mine-asteroid");

    await expect(page.locator("#asteroid-unbanked-value")).toHaveText("30");
    await expect(page.locator("#asteroid-shininess-text")).toContainText("Tier 6");

    // Fire PvP laser at Rogue Drone while outside safe zone
    await expect(page.locator("#btn-fire-laser")).toBeEnabled();
    await page.click("#btn-fire-laser");
    await expect(page.locator("#asteroid-target-hull")).toHaveText("75%");

    // Return to station dock and bank mined ore
    await page.click("#btn-fly-station");
    await expect(page.locator("#asteroid-safe-zone-badge")).toHaveText("DOCKED SAFE ZONE", {
      timeout: 6000,
    });
    await page.click("#btn-bank-loot");
    await expect(page.locator("#asteroid-vault-credits")).toHaveText("30");
    await expect(page.locator("#asteroid-unbanked-value")).toHaveText("0");

    // Disconnect and exercise Offline Playtest Workbench (Sandbox Teleport + Mine + Dummy + Bank)
    await page.click("#btn-disconnect");
    await expect(page.locator("#status-text")).toHaveText("Disconnected");

    await page.click("#btn-toggle-devtools");
    await expect(page.locator("#devtools-drawer")).toBeVisible();
    await expect(page.locator("#devtools-stats")).toContainText("Chunks: 10");

    await page.click("#btn-teleport-asteroid");
    await expect(page.locator("#val-pos")).toContainText("X: 7.5, Y: 7.5, Z: -15.5");
    for (let i = 0; i < 4; i++) {
      await page.click("#btn-mine-asteroid");
    }
    await expect(page.locator("#asteroid-unbanked-value")).not.toHaveText("0");

    await page.click("#btn-spawn-dummy");
    await expect(page.locator("#asteroid-target-panel")).toBeVisible();

    await page.click("#btn-teleport-station");
    await expect(page.locator("#asteroid-safe-zone-badge")).toHaveText("DOCKED SAFE ZONE");
    await page.click("#btn-bank-loot");
    await expect(page.locator("#asteroid-vault-credits")).not.toHaveText("0");
    await page.click("#btn-close-devtools");

    expect(pageErrors).toEqual([]);
    expect(unexpectedConsoleErrors).toEqual([]);
  });
});
