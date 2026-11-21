import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import * as THREE from "three";
import { UIController } from "../ui.js";
import {
  resolveGameMode,
  resolveGatewayConfig,
  getOrCreateSessionToken,
  autoFetchCertHash,
} from "../main.js";
import { WorldRenderer } from "../renderer.js";
import { WorldObjectRenderer } from "../world.js";
import {
  AsteroidRenderer,
  extractShininessTier,
} from "../games/asteroid/renderer.js";
import {
  AsteroidVoxelClient,
  CHUNK_VOXELS,
  VOXEL_TITANIUM,
  indexFromLocal,
} from "../games/asteroid/voxel_client.js";
import {
  installMockDocument,
  installMockWindow,
  createMerchantFixture,
  createTradeSessionFixture,
} from "./helpers/mock_dom.js";

function collectDisposables(root) {
  const geoSpies = [];
  const matSpies = [];
  const seenGeo = new Set();
  const seenMat = new Set();

  root.traverse((node) => {
    if (node.geometry && !seenGeo.has(node.geometry)) {
      seenGeo.add(node.geometry);
      geoSpies.push(vi.spyOn(node.geometry, "dispose"));
    }
    if (node.material) {
      const mats = Array.isArray(node.material) ? node.material : [node.material];
      for (const m of mats) {
        if (m && !seenMat.has(m)) {
          seenMat.add(m);
          matSpies.push(vi.spyOn(m, "dispose"));
        }
      }
    }
  });

  return { geoSpies, matSpies };
}

describe("Village NPC Barter Modal Lifecycle", () => {
  beforeEach(() => {
    installMockDocument();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("renders multi-item and currency exchange recipes and invokes recipe callbacks", () => {
    const ui = new UIController();
    const clickedRecipes = [];
    const merchant = createMerchantFixture({
      recipes: [
        {
          recipe_id: 11,
          required_items: [
            { name: "Iron Ore", quantity: 3 },
            { name: "Coal", quantity: 2 },
          ],
          required_currency: 15,
          reward_items: [
            { name: "Steel Pickaxe", quantity: 1 },
            { name: "Torch", quantity: 4 },
          ],
          reward_currency: 5,
        },
        {
          recipe_id: 12,
          required_items: [],
          required_currency: 0,
          reward_items: [{ name: "Starter Bread", quantity: 2 }],
          reward_currency: 10,
        },
      ],
    });

    ui.showNpcBarter(merchant, (recipeId) => clickedRecipes.push(recipeId));

    const modal = document.getElementById("npc-barter-modal");
    expect(modal.style.display).toBe("flex");
    expect(document.getElementById("npc-name").textContent).toBe("Blacksmith Ilmari");
    expect(document.getElementById("npc-role").textContent).toBe("Master Artisan");
    expect(document.getElementById("npc-greeting").textContent).toBe(
      "Fine steel and tools forged for hardy travelers."
    );

    const list = document.getElementById("barter-recipes-list");
    const cards = list.querySelectorAll(".barter-recipe-card");
    expect(cards).toHaveLength(2);

    const firstReqs = cards[0].querySelector(".recipe-reqs");
    const firstRewards = cards[0].querySelector(".recipe-rewards");
    expect(firstReqs.textContent).toContain("3x Iron Ore");
    expect(firstReqs.textContent).toContain("2x Coal");
    expect(firstReqs.textContent).toContain("15 Gold");
    expect(firstRewards.textContent).toContain("1x Steel Pickaxe");
    expect(firstRewards.textContent).toContain("4x Torch");
    expect(firstRewards.textContent).toContain("5 Gold");

    const secondReqs = cards[1].querySelector(".recipe-reqs");
    const secondRewards = cards[1].querySelector(".recipe-rewards");
    expect(secondReqs.textContent).toContain("Free");
    expect(secondRewards.textContent).toContain("2x Starter Bread");
    expect(secondRewards.textContent).toContain("10 Gold");

    const buttons = list.querySelectorAll(".barter-action-btn");
    expect(buttons).toHaveLength(2);
    expect(buttons[0].getAttribute("data-recipe-id")).toBe("11");
    expect(buttons[1].getAttribute("data-recipe-id")).toBe("12");

    buttons[1].click();
    buttons[0].click();
    expect(clickedRecipes).toEqual([12, 11]);
  });

  it("handles empty recipes, XSS payloads safely, and closes via hideNpcBarter or close button", () => {
    const ui = new UIController();
    ui.initEvents({});

    const xss = '<img src=x onerror="alert(1)">';
    ui.showNpcBarter(
      createMerchantFixture({
        name: xss,
        role: xss,
        greeting: xss,
        recipes: [
          {
            recipe_id: 99,
            required_items: [{ name: xss, quantity: 1 }],
            required_currency: 0,
            reward_items: [{ name: xss, quantity: 2 }],
            reward_currency: 0,
          },
        ],
      }),
      vi.fn()
    );

    const list = document.getElementById("barter-recipes-list");
    const card = list.querySelector(".barter-recipe-card");
    expect(card._innerHTML).toBe("");
    expect(card.querySelector(".recipe-reqs").textContent).toContain(`1x ${xss}`);
    expect(card.querySelector(".recipe-rewards").textContent).toContain(`2x ${xss}`);
    expect(document.getElementById("npc-name").textContent).toBe(xss);

    document.getElementById("btn-close-barter").click();
    expect(document.getElementById("npc-barter-modal").style.display).toBe("none");

    // Empty recipes fallback does not crash and renders zero barter action buttons
    ui.showNpcBarter({ name: "Elder", recipes: [] }, vi.fn());
    expect(document.getElementById("npc-barter-modal").style.display).toBe("flex");
    expect(list.querySelectorAll(".barter-action-btn")).toHaveLength(0);

    ui.hideNpcBarter();
    expect(document.getElementById("npc-barter-modal").style.display).toBe("none");
  });
});

describe("Direct Player Trade Session Lifecycle", () => {
  beforeEach(() => {
    installMockDocument();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("switches currency labels between Gold (Village) and Credits (Asteroid)", () => {
    const ui = new UIController();

    ui.setGameMode("village");
    expect(document.getElementById("trade-offer-currency-label").textContent).toBe("Offer Gold:");
    expect(document.getElementById("trade-partner-currency-label").textContent).toBe("Gold:");

    ui.setGameMode("asteroid");
    expect(document.getElementById("trade-offer-currency-label").textContent).toBe("Offer Credits:");
    expect(document.getElementById("trade-partner-currency-label").textContent).toBe("Credits:");
  });

  it("supports multi-item selection toggle, offer submission, partner items, and status transitions", () => {
    const ui = new UIController();
    const offerCalls = [];
    const acceptCalls = [];
    const cancelCalls = [];

    ui.initEvents({
      onTradeOffer: (currency, items) => offerCalls.push({ currency, items }),
      onTradeAccept: () => acceptCalls.push(true),
      onTradeCancel: () => cancelCalls.push(true),
    });

    ui.updateInventory({
      currency: 200,
      max_weight: 100.0,
      slots: [
        { item_id: 101, name: "Iron Ingot", quantity: 3, unit_weight: 1.5 },
        { item_id: 102, name: "Healing Herb", quantity: 5, unit_weight: 0.2 },
        null,
        { item_id: 103, name: "Amber Gem", quantity: 1, unit_weight: 0.1 },
      ],
    });

    ui.showPlayerTrade(
      createTradeSessionFixture({
        session_id: 303,
        their_offer: {
          currency: 75,
          items: [
            { item_id: 201, name: "Oak Bow", quantity: 1 },
            { item_id: 202, name: "<script>xss</script>", quantity: 2 },
          ],
        },
      })
    );

    expect(document.getElementById("player-trade-modal").style.display).toBe("flex");
    expect(document.getElementById("trade-session-id").textContent).toBe(303);
    expect(document.getElementById("trade-partner-gold").textContent).toBe(75);
    expect(document.getElementById("trade-status-msg").textContent).toBe("Trade active");

    const partnerChips = document
      .getElementById("trade-partner-items")
      .querySelectorAll(".trade-item-chip");
    expect(partnerChips).toHaveLength(2);
    expect(partnerChips[0].textContent).toBe("1x Oak Bow");
    expect(partnerChips[1].textContent).toBe("2x <script>xss</script>");
    expect(partnerChips[1]._innerHTML).toBe("");

    const myChips = document.getElementById("trade-my-items").querySelectorAll(".trade-item-chip");
    expect(myChips).toHaveLength(3);

    const toggle0 = myChips[0].querySelector(".trade-item-toggle");
    const toggle2 = myChips[2].querySelector(".trade-item-toggle");
    expect(toggle0).not.toBeNull();
    expect(toggle2).not.toBeNull();

    // Select item 0 and item 2, then deselect item 0
    toggle0.click();
    expect(toggle0.textContent).toBe("Offered");
    expect(toggle0.classList.contains("selected")).toBe(true);

    toggle2.click();
    expect(toggle2.textContent).toBe("Offered");

    toggle0.click();
    expect(toggle0.textContent).toBe("+ Offer");
    expect(toggle0.classList.contains("selected")).toBe(false);

    document.getElementById("trade-offer-gold").value = "40";
    document.getElementById("btn-trade-offer").click();

    expect(offerCalls).toEqual([
      {
        currency: 40,
        items: [{ item_id: 103, name: "Amber Gem", quantity: 1, unit_weight: 0.1 }],
      },
    ]);

    // Status progression: Partner locked -> Both locked -> Both accepted -> Trade completed
    ui.showPlayerTrade(createTradeSessionFixture({ session_id: 303, their_locked: true }));
    expect(document.getElementById("trade-status-msg").textContent).toBe("Partner locked");
    // Selection state persists across incoming partner state updates
    const refreshedChips = document
      .getElementById("trade-my-items")
      .querySelectorAll(".trade-item-chip");
    expect(refreshedChips[2].querySelector(".trade-item-toggle").classList.contains("selected")).toBe(
      true
    );

    ui.showPlayerTrade(
      createTradeSessionFixture({ session_id: 303, my_locked: true, their_locked: true })
    );
    expect(document.getElementById("trade-status-msg").textContent).toBe("Both locked");

    document.getElementById("btn-trade-accept").click();
    expect(acceptCalls).toHaveLength(1);

    ui.showPlayerTrade(
      createTradeSessionFixture({
        session_id: 303,
        my_locked: true,
        their_locked: true,
        my_accepted: true,
        their_accepted: true,
      })
    );
    expect(document.getElementById("trade-status-msg").textContent).toBe("Both accepted");

    ui.showPlayerTrade(createTradeSessionFixture({ session_id: 303, completed: true }));
    expect(document.getElementById("trade-status-msg").textContent).toBe("Trade completed");

    // Cancel clears selection state, hides modal, and notifies callback
    document.getElementById("btn-trade-cancel").click();
    expect(cancelCalls).toHaveLength(1);
    expect(document.getElementById("player-trade-modal").style.display).toBe("none");

    // Re-opening starts with cleared item selection
    ui.showPlayerTrade(createTradeSessionFixture({ session_id: 304 }));
    const resetChips = document
      .getElementById("trade-my-items")
      .querySelectorAll(".trade-item-chip");
    expect(resetChips[2].querySelector(".trade-item-toggle").classList.contains("selected")).toBe(
      false
    );

    // Close button also hides and triggers cancel callback
    document.getElementById("btn-close-trade").click();
    expect(cancelCalls).toHaveLength(2);
    expect(document.getElementById("player-trade-modal").style.display).toBe("none");
  });

  it("hides village barter and target overlays when switching to asteroid mode", () => {
    const ui = new UIController();
    ui.showTarget("Blacksmith Ilmari", "Villager NPC", 2.1);
    ui.showNpcBarter(createMerchantFixture(), vi.fn());
    expect(document.getElementById("npc-barter-modal").style.display).toBe("flex");
    expect(document.getElementById("target-panel").style.display).toBe("block");

    ui.setGameMode("asteroid");
    expect(document.getElementById("npc-barter-modal").style.display).toBe("none");
    expect(document.getElementById("target-panel").style.display).toBe("none");

    ui.showAsteroidTarget({ id: 55, distance: 12.0, hpPct: 0.8, stateFlags: 10 << 16 }, false);
    expect(document.getElementById("asteroid-target-panel").style.display).toBe("block");

    ui.setGameMode("village");
    expect(document.getElementById("asteroid-target-panel").style.display).toBe("none");
  });
});

describe("Subpath Routing, Gateway Endpoints, Token Isolation, and Cert Discovery", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("resolves query, subpath, and env priorities across village and asteroid aliases", () => {
    expect(resolveGameMode("", "", "/")).toBe("village");
    expect(resolveGameMode("", "", "/village/")).toBe("village");
    expect(resolveGameMode("", "asteroid", "/village")).toBe("village");
    expect(resolveGameMode("", "", "/asteroid")).toBe("asteroid");
    expect(resolveGameMode("", "", "/play/asteroid/sector-1")).toBe("asteroid");
    expect(resolveGameMode("", "", "/game-b")).toBe("asteroid");
    expect(resolveGameMode("?game=village", "asteroid", "/asteroid")).toBe("village");
    expect(resolveGameMode("?game=asteroid", "village", "/village")).toBe("asteroid");
    expect(resolveGameMode("?game=unknown", "asteroid", "/unknown")).toBe("asteroid");
  });

  it("derives http/ws vs https/wss gateway configs cleanly per game mode", () => {
    const httpLoc = { hostname: "127.0.0.1", host: "127.0.0.1:3000", protocol: "http:" };
    expect(resolveGatewayConfig("village", httpLoc)).toEqual({
      mode: "village",
      subpath: "/village",
      wtUrl: "https://127.0.0.1:4433",
      wsPathUrl: "ws://127.0.0.1:3000/village/ws",
      wsPortUrl: "ws://127.0.0.1:8443",
      wsPort: 8443,
      adminPort: 9091,
      certHashPaths: ["/village/cert-hash", "/cert-hash"],
    });

    expect(resolveGatewayConfig("asteroid", httpLoc)).toEqual({
      mode: "asteroid",
      subpath: "/asteroid",
      wtUrl: "https://127.0.0.1:4434",
      wsPathUrl: "ws://127.0.0.1:3000/asteroid/ws",
      wsPortUrl: "ws://127.0.0.1:8444",
      wsPort: 8444,
      adminPort: 9092,
      certHashPaths: ["/asteroid/cert-hash"],
    });
  });

  it("isolates session tokens across village and asteroid modes", () => {
    expect(getOrCreateSessionToken("village")).toBe("guest-poc");
    expect(getOrCreateSessionToken("asteroid")).toBe("guest-poc-asteroid");

    const { window: win } = installMockWindow({ search: "" });
    const villageToken = getOrCreateSessionToken("village");
    const asteroidToken = getOrCreateSessionToken("asteroid");

    expect(villageToken).toMatch(/^guest-[a-z0-9]+-[a-z0-9]+$/);
    expect(asteroidToken).toBe(`${villageToken}-asteroid`);
    expect(asteroidToken).not.toBe(villageToken);

    // Query parameter override also remains isolated across modes
    win.location.search = "?token=pilot-zero";
    expect(getOrCreateSessionToken("village")).toBe("pilot-zero");
    expect(getOrCreateSessionToken("asteroid")).toBe("pilot-zero-asteroid");
  });

  it("recovers when cert-hash subpaths return SPA HTML or 404 before JSON succeeds", async () => {
    installMockDocument();
    installMockWindow({
      hostname: "game.example.test",
      host: "game.example.test",
      protocol: "https:",
      pathname: "/village",
    });

    const requestedUrls = [];
    const mockFetch = vi.fn(async (url) => {
      requestedUrls.push(url);
      if (url === "/village/cert-hash") {
        // Simulates SPA fallback returning index.html with 200 OK
        return {
          ok: true,
          status: 200,
          json: async () => {
            throw new SyntaxError("Unexpected token '<', \"<!DOCTYPE \"... is not valid JSON");
          },
        };
      }
      if (url === "/cert-hash") {
        return { ok: false, status: 404, json: async () => ({}) };
      }
      if (url === "https://game.example.test:9091/cert-hash") {
        return {
          ok: true,
          status: 200,
          json: async () => ({ cert_hash: "aa:bb:cc:dd:11:22" }),
        };
      }
      throw new Error("unexpected URL");
    });
    vi.stubGlobal("fetch", mockFetch);

    await autoFetchCertHash("village");

    expect(requestedUrls).toEqual([
      "/village/cert-hash",
      "/cert-hash",
      "https://game.example.test:9091/cert-hash",
    ]);
    expect(document.getElementById("cert-hash").value).toBe("aa:bb:cc:dd:11:22");

    // force=false preserves already-populated cert-hash without fetching
    mockFetch.mockClear();
    await autoFetchCertHash("asteroid", false);
    expect(mockFetch).not.toHaveBeenCalled();
    expect(document.getElementById("cert-hash").value).toBe("aa:bb:cc:dd:11:22");

    // force=true clears stale village hash and queries asteroid endpoints even if all fail
    mockFetch.mockImplementation(async (url) => {
      requestedUrls.push(url);
      return {
        ok: true,
        json: async () => {
          throw new SyntaxError("Unexpected HTML");
        },
      };
    });
    requestedUrls.length = 0;

    await expect(autoFetchCertHash("asteroid", true)).resolves.toBeUndefined();
    expect(document.getElementById("cert-hash").value).toBe("");
    expect(requestedUrls).toEqual([
      "/asteroid/cert-hash",
      "https://game.example.test:9092/cert-hash",
      "http://game.example.test:9092/cert-hash",
    ]);

    // Ignores responses with missing or non-string cert_hash without throwing
    mockFetch.mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ cert_hash: 12345 }),
    });
    await expect(autoFetchCertHash("village", true)).resolves.toBeUndefined();
    expect(document.getElementById("cert-hash").value).toBe("");
  });
});

describe("3D Renderer Mesh Invariants & GPU Resource Cleanup", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("maintains WorldRenderer entity invariants and disposes entity and prefab hierarchies on clear()", () => {
    const worldRenderer = new WorldRenderer(null);
    const buf = new Float32Array(18);
    const u32 = new Uint32Array(buf.buffer);

    // Local player (guid = 1, type = 1)
    u32[0] = 1;
    u32[1] = 0;
    buf[2] = 0;
    buf[3] = 0;
    buf[4] = 5;
    buf[5] = 0.5;
    buf[6] = 0.8;
    buf[7] = 0;
    buf[8] = 1;

    // Villager NPC (guid = (2n << 48n) | 7n, type = 2)
    const npcGuid = (2n << 48n) | 7n;
    u32[9] = Number(npcGuid & 0xffffffffn) >>> 0;
    u32[10] = Number((npcGuid >> 32n) & 0xffffffffn) >>> 0;
    buf[11] = 2.0;
    buf[12] = 0.0;
    buf[13] = 3.0;
    buf[14] = 1.2;
    buf[15] = 0.4;
    buf[16] = 0;
    buf[17] = 2;

    worldRenderer.syncEntities(buf, 1);
    expect(worldRenderer.entities.size).toBe(2);

    const localGroup = worldRenderer.entities.get(1n);
    const npcGroup = worldRenderer.entities.get(npcGuid);
    expect(localGroup).toBeInstanceOf(THREE.Group);
    expect(localGroup.userData.hpBar).toBeDefined();
    expect(localGroup.userData.hpBar.scale.x).toBeCloseTo(0.8, 4);
    expect(localGroup.userData.marker).toBeUndefined();

    expect(npcGroup).toBeInstanceOf(THREE.Group);
    expect(npcGroup.userData.hpBar).toBeDefined();
    expect(npcGroup.userData.hpBar.scale.x).toBeCloseTo(0.4, 4);
    expect(npcGroup.userData.marker).toBeDefined();
    expect(npcGroup.userData.marker.material.color.getHex()).toBe(0xf59e0b);

    // Populate village world object prefabs 10..13
    const objBuf = new Float32Array(16 * 4);
    const objU32 = new Uint32Array(objBuf.buffer);
    [10, 11, 12, 13].forEach((prefabId, idx) => {
      const base = idx * 16;
      objU32[base] = idx + 100;
      objBuf[base + 1] = prefabId;
      objBuf[base + 8] = 1.0; // qw
      objBuf[base + 9] = 1.0; // sx
      objBuf[base + 10] = 1.0; // sy
      objBuf[base + 11] = 1.0; // sz
      objBuf[base + 13] = 0.75; // hpPct
      objU32[base + 15] = 0;
    });
    worldRenderer.updateWorldObjects(objBuf);
    expect(worldRenderer.worldObjects.objects.size).toBe(4);

    for (let idx = 0; idx < 4; idx++) {
      const objGroup = worldRenderer.worldObjects.objects.get(BigInt(idx + 100));
      expect(objGroup).toBeInstanceOf(THREE.Group);
      expect(objGroup.userData.mesh).toBeDefined();
      expect(objGroup.userData.hpBar).toBeDefined();
      expect(objGroup.userData.hpBar.visible).toBe(true);
      expect(objGroup.userData.hpBar.scale.x).toBeCloseTo(0.75, 4);
    }

    const trackedGroups = [
      localGroup,
      npcGroup,
      ...Array.from(worldRenderer.worldObjects.objects.values()),
    ];
    const geoSpies = [];
    const matSpies = [];
    for (const grp of trackedGroups) {
      const collected = collectDisposables(grp);
      geoSpies.push(...collected.geoSpies);
      matSpies.push(...collected.matSpies);
    }
    expect(geoSpies.length).toBeGreaterThanOrEqual(6);
    expect(matSpies.length).toBeGreaterThanOrEqual(6);

    worldRenderer.clear();

    expect(worldRenderer.entities.size).toBe(0);
    expect(worldRenderer.worldObjects.objects.size).toBe(0);
    for (const grp of trackedGroups) {
      expect(worldRenderer.scene.children).not.toContain(grp);
    }
    for (const spy of geoSpies) expect(spy).toHaveBeenCalled();
    for (const spy of matSpies) expect(spy).toHaveBeenCalled();

    if (typeof worldRenderer.dispose === "function") {
      expect(() => worldRenderer.dispose()).not.toThrow();
    }
  });

  it("toggles WorldObjectRenderer damaged state and disposes composite prefab meshes", () => {
    const scene = new THREE.Scene();
    const objRenderer = new WorldObjectRenderer(scene);

    const buf = new Float32Array(16);
    const u32 = new Uint32Array(buf.buffer);
    u32[0] = 77;
    buf[1] = 12; // Market Stall
    buf[8] = 1.0;
    buf[9] = 1.0;
    buf[10] = 1.0;
    buf[11] = 1.0;
    buf[12] = 0; // intact
    buf[13] = -1.0; // negative hpPct hides bar
    u32[15] = 0;

    objRenderer.updateFromBuffer(buf);
    const group = objRenderer.objects.get(77n);
    expect(group.userData.mesh.material).toBe(objRenderer.materials.stall);
    expect(group.userData.hpBar.visible).toBe(false);

    // Apply DAMAGED flag (16) and visible hpPct
    buf[12] = 16;
    buf[13] = 0.25;
    objRenderer.updateFromBuffer(buf);
    expect(group.userData.mesh.material).toBe(objRenderer.materials.damaged);
    expect(group.userData.hpBar.visible).toBe(true);
    expect(group.userData.hpBar.scale.x).toBeCloseTo(0.25, 4);

    const { geoSpies, matSpies } = collectDisposables(group);
    objRenderer.clear();
    expect(objRenderer.objects.size).toBe(0);
    expect(scene.children).toHaveLength(0);
    for (const spy of geoSpies) expect(spy).toHaveBeenCalled();
    for (const spy of matSpies) expect(spy).toHaveBeenCalled();
  });

  it("disposes AsteroidRenderer ships, voxel chunks, laser beams, and materials on clear() and dispose()", () => {
    const renderer = new AsteroidRenderer(null);
    renderer.initEnvironment();

    const entitiesMap = new Map();
    const remoteGuid = (1n << 48n) | 99n;
    entitiesMap.set(Number(remoteGuid), {
      stateFlags: 180 << 16,
      pitch: 0.15,
      hpPct: 0.65,
    });

    const buf = new Float32Array(18);
    const u32 = new Uint32Array(buf.buffer);
    u32[0] = 1;
    u32[1] = 0;
    buf[6] = 1.0;
    buf[8] = 1;

    u32[9] = Number(remoteGuid & 0xffffffffn) >>> 0;
    u32[10] = Number((remoteGuid >> 32n) & 0xffffffffn) >>> 0;
    buf[11] = 10;
    buf[12] = 2;
    buf[13] = -12;
    buf[14] = 0.8;
    buf[15] = 0.65;
    buf[17] = 1;

    renderer.syncEntities(buf, 1, entitiesMap, { pitch: -0.1, stateFlags: 64 << 16 });
    const remoteShip = renderer.entities.get(remoteGuid);
    expect(remoteShip.userData.hullMat).toBeDefined();
    expect(remoteShip.userData.auraHalo).toBeDefined();
    expect(remoteShip.userData.auraLight).toBeDefined();
    expect(remoteShip.userData.hpBar).toBeDefined();
    expect(extractShininessTier(remoteShip.userData.stateFlags)).toBe(180);
    expect(remoteShip.userData.hpBar.scale.x).toBeCloseTo(0.65, 4);

    const voxelClient = new AsteroidVoxelClient();
    const voxels = new Uint16Array(CHUNK_VOXELS);
    voxels[indexFromLocal(1, 1, 1)] = VOXEL_TITANIUM;
    voxelClient.setChunk(0, 0, -1, voxels, 1);
    renderer.syncVoxelChunks(voxelClient);
    expect(renderer.voxelChunkMeshes.size).toBe(1);

    const beam = renderer.spawnLaserBeam({ x: 0, y: 0, z: 8 }, { x: 10, y: 2, z: -12 }, 0xf43f5e, 300);
    expect(beam).not.toBeNull();
    expect(renderer.laserBeams).toHaveLength(1);

    const shipSpies = collectDisposables(remoteShip);
    const chunkMesh = renderer.voxelChunkMeshes.get("0,0,-1");
    const chunkGeoDispose = vi.spyOn(chunkMesh.geometry, "dispose");
    const beamSpies = collectDisposables(beam);

    renderer.clear();
    expect(renderer.entities.size).toBe(0);
    expect(renderer.voxelChunkMeshes.size).toBe(0);
    expect(renderer.laserBeams).toHaveLength(0);

    for (const spy of shipSpies.geoSpies) expect(spy).toHaveBeenCalled();
    for (const spy of shipSpies.matSpies) expect(spy).toHaveBeenCalled();
    expect(chunkGeoDispose).toHaveBeenCalledTimes(1);
    for (const spy of beamSpies.geoSpies) expect(spy).toHaveBeenCalled();
    for (const spy of beamSpies.matSpies) expect(spy).toHaveBeenCalled();

    const voxelMatDispose = vi.spyOn(renderer.voxelMaterial, "dispose");
    const envSpies = collectDisposables(renderer.scene);
    renderer.dispose();

    expect(voxelMatDispose).toHaveBeenCalled();
    for (const spy of envSpies.geoSpies) expect(spy).toHaveBeenCalled();
    for (const spy of envSpies.matSpies) expect(spy).toHaveBeenCalled();
  });

  it("expires transient AsteroidRenderer laser beams during render() and disposes their resources", () => {
    let nowMs = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => nowMs);

    const renderer = new AsteroidRenderer(null);
    const beam = renderer.spawnLaserBeam(
      { x: 0, y: 0, z: 0 },
      { x: 0, y: 0, z: -15 },
      0x38bdf8,
      200
    );
    expect(beam).not.toBeNull();
    expect(renderer.laserBeams).toHaveLength(1);

    const beamSpies = collectDisposables(beam);
    nowMs = 1100; // Before expiry
    renderer.render();
    expect(renderer.laserBeams).toHaveLength(1);

    nowMs = 1250; // Past expiry
    renderer.render();
    expect(renderer.laserBeams).toHaveLength(0);
    expect(renderer.scene.children).not.toContain(beam);
    for (const spy of beamSpies.geoSpies) expect(spy).toHaveBeenCalled();
    for (const spy of beamSpies.matSpies) expect(spy).toHaveBeenCalled();
  });
});
