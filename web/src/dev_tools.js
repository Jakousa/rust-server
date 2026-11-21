import {
  VOXEL_EMPTY,
  VOXEL_TITANIUM,
  VOXEL_SOLAR_GOLD,
  VOXEL_VOID_CRYSTAL,
  getVoxelMaterialInfo,
} from "./games/asteroid/voxel_client.js";
import { HOME_STATION_POS, HOME_STATION_SAFE_RADIUS } from "./games/asteroid/renderer.js";

const SANDBOX_SLOT_COUNT = 16;
const SANDBOX_MAX_WEIGHT = 150.0;
const SANDBOX_LASER_RANGE = 45.0;
const SANDBOX_LASER_DAMAGE = 25.0;
const SANDBOX_LASER_COOLDOWN_MS = 500;

function isSafeZone(pos) {
  const dx = (pos?.x ?? 0) - HOME_STATION_POS.x;
  const dy = (pos?.y ?? 0) - HOME_STATION_POS.y;
  const dz = (pos?.z ?? 0) - HOME_STATION_POS.z;
  return Math.hypot(dx, dy, dz) <= HOME_STATION_SAFE_RADIUS;
}

function createOreStackForVoxel(voxelId) {
  if (voxelId === VOXEL_TITANIUM) {
    return { item_id: 2001, name: "Titanium Ore", quantity: 1, unit_weight: 2.0 };
  }
  if (voxelId === VOXEL_SOLAR_GOLD) {
    return { item_id: 2002, name: "Solar Gold", quantity: 1, unit_weight: 3.0 };
  }
  if (voxelId === VOXEL_VOID_CRYSTAL) {
    return { item_id: 2003, name: "Void Crystal", quantity: 1, unit_weight: 1.0 };
  }
  return null;
}

function oreValueByItemId(itemId) {
  if (itemId === 2001) return 10;
  if (itemId === 2002) return 30;
  if (itemId === 2003) return 75;
  return 0;
}

function computeInventoryWeight(inv) {
  let total = 0;
  for (const s of inv.slots) {
    if (s) total += (Number(s.unit_weight) || 0) * (Number(s.quantity) || 0);
  }
  return total;
}

function computeCargoValue(inv) {
  let total = Number(inv.currency || 0);
  for (const s of inv.slots) {
    if (s) total += oreValueByItemId(s.item_id) * (Number(s.quantity) || 0);
  }
  return total;
}

function mergeStackRespectingCapacity(inv, stack) {
  if (!stack || stack.quantity <= 0) return 0;
  const available = Math.max(0, inv.max_weight - computeInventoryWeight(inv));
  const maxByWeight =
    stack.unit_weight <= 0 ? stack.quantity : Math.floor(available / stack.unit_weight);
  const addQty = Math.min(stack.quantity, maxByWeight);
  if (addQty <= 0) return 0;

  const existing = inv.slots.find((s) => s && s.item_id === stack.item_id);
  if (existing) {
    existing.quantity += addQty;
    return addQty;
  }

  const emptyIdx = inv.slots.findIndex((s) => !s);
  if (emptyIdx === -1) return 0;
  inv.slots[emptyIdx] = {
    item_id: stack.item_id,
    name: stack.name,
    quantity: addQty,
    unit_weight: stack.unit_weight,
  };
  return addQty;
}

/**
 * In-browser Playtest Workbench & isolated offline sandbox simulator for rapid iteration
 * on `/asteroid` without requiring container rebuilds or live cluster connectivity.
 */
export class DevToolsController {
  constructor({ world = null, voxelClient = null, onStateUpdate = null, onToast = null } = {}) {
    this.world = world;
    this.voxelClient = voxelClient;
    this.onStateUpdate = onStateUpdate;
    this.onToast = onToast;

    this.isOpen = false;
    this.sandboxEnabled = false;
    this.dummySeq = 0;
    this.dummies = new Map();
    this.lastLaserAtMs = 0;
    this.sandboxState = this._createInitialState();
  }

  _createInitialState() {
    return {
      hull_hp: 100,
      max_hull_hp: 100,
      inventory: {
        currency: 0,
        max_weight: SANDBOX_MAX_WEIGHT,
        slots: Array(SANDBOX_SLOT_COUNT).fill(null),
      },
      station_vault: {
        banked_credits: 0,
        banked_items: [],
        kills: 0,
        deaths: 0,
      },
    };
  }

  /**
   * Toggles Local Sandbox Mode (isolated from live server transport).
   */
  setSandboxEnabled(enabled, { silent = false } = {}) {
    const next = Boolean(enabled);
    if (this.sandboxEnabled === next) return next;
    this.sandboxEnabled = next;
    if (!next) {
      this.clearDummies();
    } else if (!silent && this.onToast) {
      this.onToast("Local Sandbox Mode enabled (offline mining & combat active).", "info", 2400);
    }
    return next;
  }

  /**
   * Automatically exits sandbox isolation when an authenticated live session connects.
   */
  onLiveAuthSucceeded() {
    this.clearDummies();
    this.sandboxEnabled = false;
  }

  resetSandbox(shipPos = { x: 0, y: 0, z: 8 }) {
    this.clearDummies();
    this.sandboxState = this._createInitialState();
    this.voxelClient?.seedLocalPreviewChunks?.(true);
    const resp = this.buildSandboxStateResponse(shipPos);
    if (this.onStateUpdate) this.onStateUpdate(resp);
    return resp;
  }

  clearDummies() {
    if (this.world?.entities) {
      for (const guid of this.dummies.keys()) {
        this.world.entities.delete(guid);
        this.world.interpHistory?.delete?.(guid);
      }
    }
    this.dummies.clear();
  }

  /**
   * Spawns a targetable shiny dummy drone/ship in `world.entities` for offline PvP testing.
   */
  spawnTargetDummy(pos = { x: 14.0, y: 0.0, z: -18.0 }, tier = 200) {
    if (!this.sandboxEnabled) {
      this.setSandboxEnabled(true, { silent: true });
    }
    this.dummySeq += 1;
    const idNum = 9000 + this.dummySeq;
    const guid = (2n << 48n) | BigInt(idNum);
    const entKey = Number(guid);
    const clampedTier = Math.max(0, Math.min(255, Number(tier) || 0));
    const dummy = {
      id: entKey,
      x: pos.x,
      y: pos.y,
      z: pos.z,
      serverX: pos.x,
      serverY: pos.y,
      serverZ: pos.z,
      vx: 0,
      vy: 0,
      vz: 0,
      yaw: Math.PI,
      serverYaw: Math.PI,
      pitch: 0,
      health: 100,
      maxHealth: 100,
      hpPct: 1.0,
      castPct: 0.0,
      stateFlags: clampedTier << 16,
      type: 2,
      isSandboxDummy: true,
      lootItems: [
        { item_id: 2002, name: "Solar Gold", quantity: 2, unit_weight: 3.0 },
        { item_id: 2003, name: "Void Crystal", quantity: 1, unit_weight: 1.0 },
      ],
    };
    this.dummies.set(entKey, dummy);
    if (this.world?.entities) {
      this.world.entities.set(entKey, dummy);
    }
    return dummy;
  }

  /**
   * Mines a voxel locally in Sandbox Mode, crediting ore and updating shininess.
   */
  mineVoxelLocally(vx, vy, vz, shipPos = { x: 0, y: 0, z: 8 }) {
    if (!this.sandboxEnabled || !this.voxelClient) return null;
    const voxelId = this.voxelClient.getVoxel(vx, vy, vz);
    if (voxelId === VOXEL_EMPTY) return null;

    this.voxelClient.setVoxel(vx, vy, vz, VOXEL_EMPTY);
    const oreStack = createOreStackForVoxel(voxelId);
    const matInfo = getVoxelMaterialInfo(voxelId);
    let harvested = false;
    if (oreStack) {
      harvested = mergeStackRespectingCapacity(this.sandboxState.inventory, oreStack) > 0;
    }

    const resp = this.buildSandboxStateResponse(shipPos);
    if (this.onStateUpdate) this.onStateUpdate(resp);
    if (this.onToast) {
      if (oreStack && !harvested) {
        this.onToast("Cargo hold full! Dock at station (0,0,0) to bank ore.", "warning", 2200);
      } else if (oreStack) {
        this.onToast(`Harvested 1x ${oreStack.name} (+${matInfo.unitValue} cr)`, "success", 1600);
      } else {
        this.onToast(`Cleared ${matInfo.name} crust`, "info", 1100);
      }
    }
    return resp;
  }

  /**
   * Fires a simulated PvP laser at a sandbox dummy with range, safe-zone, and full-loot rules.
   */
  fireSandboxLaser(targetId, attackerPos, nowMs = Date.now()) {
    if (!this.sandboxEnabled) return { ok: false, error: "Sandbox mode is disabled" };
    const dummy = this.dummies.get(targetId);
    if (!dummy) return { ok: false, error: "No sandbox target locked" };

    if (isSafeZone(attackerPos)) {
      return { ok: false, error: "Cannot fire PvP laser inside Home Orbital Station safe zone" };
    }
    if (isSafeZone(dummy)) {
      return { ok: false, error: "Target ship is protected inside Home Orbital Station safe zone" };
    }
    const dist = Math.hypot(
      dummy.x - attackerPos.x,
      dummy.y - attackerPos.y,
      dummy.z - attackerPos.z
    );
    if (dist > SANDBOX_LASER_RANGE) {
      return { ok: false, error: `Target out of laser range (${dist.toFixed(1)}m > 45.0m)` };
    }
    if (nowMs - this.lastLaserAtMs < SANDBOX_LASER_COOLDOWN_MS) {
      return { ok: false, error: "PvP laser recharging..." };
    }
    this.lastLaserAtMs = nowMs;

    dummy.health = Math.max(0, dummy.health - SANDBOX_LASER_DAMAGE);
    dummy.hpPct = dummy.health / dummy.maxHealth;

    let destroyed = false;
    if (dummy.health <= 0) {
      destroyed = true;
      for (const stack of dummy.lootItems) {
        mergeStackRespectingCapacity(this.sandboxState.inventory, stack);
      }
      this.sandboxState.station_vault.kills += 1;
      this.dummies.delete(targetId);
      this.world?.entities?.delete?.(targetId);
    }

    const resp = this.buildSandboxStateResponse(attackerPos);
    if (this.onStateUpdate) this.onStateUpdate(resp);
    return { ok: true, destroyed, remainingHp: dummy.health, state: resp };
  }

  /**
   * Banks unbanked sandbox cargo into the permanent station vault when inside 15m safe zone.
   */
  bankSandboxCargo(shipPos) {
    if (!this.sandboxEnabled) return null;
    if (!isSafeZone(shipPos)) {
      const errResp = this.buildSandboxStateResponse(
        shipPos,
        `Must dock within ${HOME_STATION_SAFE_RADIUS.toFixed(1)}m of Home Orbital Station to deposit cargo`
      );
      if (this.onStateUpdate) this.onStateUpdate(errResp);
      return errResp;
    }

    const value = computeCargoValue(this.sandboxState.inventory);
    for (let i = 0; i < this.sandboxState.inventory.slots.length; i++) {
      const slot = this.sandboxState.inventory.slots[i];
      if (!slot) continue;
      const existing = this.sandboxState.station_vault.banked_items.find(
        (b) => b.item_id === slot.item_id
      );
      if (existing) {
        existing.quantity += slot.quantity;
      } else {
        this.sandboxState.station_vault.banked_items.push({ ...slot });
      }
      this.sandboxState.inventory.slots[i] = null;
    }
    this.sandboxState.inventory.currency = 0;
    this.sandboxState.station_vault.banked_credits += value;

    const resp = this.buildSandboxStateResponse(shipPos);
    if (this.onStateUpdate) this.onStateUpdate(resp);
    return resp;
  }

  buildSandboxStateResponse(shipPos = { x: 0, y: 0, z: 8 }, error = null) {
    const unbankedVal = computeCargoValue(this.sandboxState.inventory);
    const shininessTier = Math.min(255, Math.floor(unbankedVal / 5));
    const currentWeight = computeInventoryWeight(this.sandboxState.inventory);
    return {
      player_guid: this.world?.localPlayerGuid ?? 1,
      inventory: {
        currency: this.sandboxState.inventory.currency,
        max_weight: this.sandboxState.inventory.max_weight,
        slots: this.sandboxState.inventory.slots.map((s) => (s ? { ...s } : null)),
      },
      current_weight: currentWeight,
      max_weight: this.sandboxState.inventory.max_weight,
      station_vault: {
        banked_credits: this.sandboxState.station_vault.banked_credits,
        banked_items: this.sandboxState.station_vault.banked_items.map((b) => ({ ...b })),
        kills: this.sandboxState.station_vault.kills,
        deaths: this.sandboxState.station_vault.deaths,
      },
      hull_hp: this.sandboxState.hull_hp,
      max_hull_hp: this.sandboxState.max_hull_hp,
      shininess_tier: shininessTier,
      unbanked_cargo_value: unbankedVal,
      in_safe_zone: isSafeZone(shipPos),
      active_trade: null,
      target_info: null,
      trade_completed: false,
      error,
    };
  }

  getDiagnosticsSummary(shipPos = { x: 0, y: 0, z: 8 }) {
    const stats = this.voxelClient?.getLoadedStats?.() ?? {
      chunkCount: 0,
      authoritativeCount: 0,
      previewCount: 0,
      totalSolidVoxels: 0,
      totalOreVoxels: 0,
      pendingRequests: 0,
    };
    const clusters = this.voxelClient?.getAsteroidClusterSummaries?.(shipPos) ?? [];
    const nearest = clusters[0] || null;
    return {
      ...stats,
      nearestAsteroidDistance: nearest ? nearest.distance : null,
      nearestAsteroidCenter: nearest ? nearest.center : null,
      sandboxEnabled: this.sandboxEnabled,
      dummyCount: this.dummies.size,
    };
  }
}
