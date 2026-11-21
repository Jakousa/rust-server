/**
 * HUD Telemetry & User Interface Controller.
 */
export class UIController {
  constructor() {
    this.statusDot = document.getElementById("status-dot");
    this.statusText = document.getElementById("status-text");
    this.transportBadge = document.getElementById("transport-badge");
    this.hudToast = document.getElementById("hud-toast");
    this._toastTimer = null;
    this._lastTransportMode = null;
    this.gatewayUrlInput = document.getElementById("gateway-url");
    this.certHashInput = document.getElementById("cert-hash");
    this.btnConnect = document.getElementById("btn-connect");
    this.btnDisconnect = document.getElementById("btn-disconnect");
    this.connectionError = document.getElementById("connection-error");

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

    // Game Mode Switcher & Controls Hint
    this.gameMode = "village";
    this.btnModeVillage = document.getElementById("btn-mode-village");
    this.btnModeAsteroid = document.getElementById("btn-mode-asteroid");
    this.villageHud = document.getElementById("village-hud");
    this.asteroidHud = document.getElementById("asteroid-hud");
    this.controlsHintTitle = document.getElementById("controls-hint-title");
    this.controlsHintGrid = document.getElementById("controls-hint-grid");
    this.controlsHintSubtext = document.getElementById("controls-hint-subtext");

    // Village HUD & Modals
    this.valGold = document.getElementById("val-gold");
    this.valWeight = document.getElementById("val-weight");
    this.inventoryGrid = document.getElementById("inventory-grid");

    // Asteroid Miners HUD
    this.asteroidSafeZoneBadge = document.getElementById("asteroid-safe-zone-badge");
    this.asteroidHullText = document.getElementById("asteroid-hull-text");
    this.asteroidHullBar = document.getElementById("asteroid-hull-bar");
    this.asteroidShininessText = document.getElementById("asteroid-shininess-text");
    this.asteroidShininessBar = document.getElementById("asteroid-shininess-bar");
    this.asteroidKdStats = document.getElementById("asteroid-kd-stats");
    this.btnMineAsteroid = document.getElementById("btn-mine-asteroid");
    this.btnBankLoot = document.getElementById("btn-bank-loot");
    this.asteroidUnbankedValue = document.getElementById("asteroid-unbanked-value");
    this.asteroidCargoWeight = document.getElementById("asteroid-cargo-weight");
    this.asteroidCargoGrid = document.getElementById("asteroid-cargo-grid");
    this.asteroidVaultCredits = document.getElementById("asteroid-vault-credits");
    this.asteroidVaultItems = document.getElementById("asteroid-vault-items");
    this.asteroidTargetPanel = document.getElementById("asteroid-target-panel");
    this.asteroidTargetName = document.getElementById("asteroid-target-name");
    this.asteroidTargetShininess = document.getElementById("asteroid-target-shininess");
    this.asteroidTargetDistance = document.getElementById("asteroid-target-distance");
    this.asteroidTargetHull = document.getElementById("asteroid-target-hull");
    this.btnFireLaser = document.getElementById("btn-fire-laser");
    this.btnTradePilot = document.getElementById("btn-trade-pilot");

    this.targetPanel = document.getElementById("target-panel");
    this.targetName = document.getElementById("target-name");
    this.targetType = document.getElementById("target-type");
    this.targetDistance = document.getElementById("target-distance");
    this.btnInteract = document.getElementById("btn-interact");

    this.npcBarterModal = document.getElementById("npc-barter-modal");
    this.npcName = document.getElementById("npc-name");
    this.npcRole = document.getElementById("npc-role");
    this.npcGreeting = document.getElementById("npc-greeting");
    this.barterRecipesList = document.getElementById("barter-recipes-list");
    this.btnCloseBarter = document.getElementById("btn-close-barter");

    this.playerTradeModal = document.getElementById("player-trade-modal");
    this.tradeSessionId = document.getElementById("trade-session-id");
    this.tradeOfferCurrencyLabel = document.getElementById("trade-offer-currency-label");
    this.tradePartnerCurrencyLabel = document.getElementById("trade-partner-currency-label");
    this.tradeOfferGold = document.getElementById("trade-offer-gold");
    this.tradeMyItems = document.getElementById("trade-my-items");
    this.tradePartnerGold = document.getElementById("trade-partner-gold");
    this.tradePartnerItems = document.getElementById("trade-partner-items");
    this.tradeStatusMsg = document.getElementById("trade-status-msg");
    this.btnTradeOffer = document.getElementById("btn-trade-offer");
    this.btnTradeAccept = document.getElementById("btn-trade-accept");
    this.btnTradeCancel = document.getElementById("btn-trade-cancel");
    this.btnCloseTrade = document.getElementById("btn-close-trade");

    this._lastInventorySlots = [];
    this._selectedTradeItems = new Map();

    this.frameCount = 0;
    this.lastFpsUpdate = performance.now();
  }

  initEvents({
    onConnect,
    onDisconnect,
    onTogglePartitions,
    onToggleAoi,
    onToggleWireframe,
    onInteract,
    onTradeOffer,
    onTradeAccept,
    onTradeCancel,
    onGameModeChange,
    onMineAsteroid,
    onFireLaser,
    onBankLoot,
    onTradePilot,
  }) {
    this.btnModeVillage?.addEventListener("click", () => {
      if (onGameModeChange) onGameModeChange("village");
    });

    this.btnModeAsteroid?.addEventListener("click", () => {
      if (onGameModeChange) onGameModeChange("asteroid");
    });

    this.btnMineAsteroid?.addEventListener("click", () => {
      if (onMineAsteroid) onMineAsteroid();
    });

    this.btnBankLoot?.addEventListener("click", () => {
      if (onBankLoot) onBankLoot();
    });

    this.btnFireLaser?.addEventListener("click", () => {
      if (onFireLaser) onFireLaser();
    });

    this.btnTradePilot?.addEventListener("click", () => {
      if (onTradePilot) onTradePilot();
    });

    this.btnConnect?.addEventListener("click", () => {
      const url = this.gatewayUrlInput.value;
      const certHash = this.certHashInput.value;
      if (onConnect) onConnect(url, certHash);
    });

    this.btnDisconnect?.addEventListener("click", () => {
      if (onDisconnect) onDisconnect();
    });

    this.togglePartitions?.addEventListener("change", (e) => {
      if (onTogglePartitions) onTogglePartitions(e.target.checked);
    });

    this.toggleAoi?.addEventListener("change", (e) => {
      if (onToggleAoi) onToggleAoi(e.target.checked);
    });

    this.toggleWireframe?.addEventListener("change", (e) => {
      if (onToggleWireframe) onToggleWireframe(e.target.checked);
    });

    this.btnInteract?.addEventListener("click", () => {
      if (onInteract) onInteract();
    });

    this.btnCloseBarter?.addEventListener("click", () => {
      this.hideNpcBarter();
    });

    this.btnCloseTrade?.addEventListener("click", () => {
      this.hidePlayerTrade();
      if (onTradeCancel) onTradeCancel();
    });

    this.btnTradeCancel?.addEventListener("click", () => {
      this.hidePlayerTrade();
      if (onTradeCancel) onTradeCancel();
    });

    this.btnTradeOffer?.addEventListener("click", () => {
      const gold = Number(this.tradeOfferGold?.value || 0);
      const items = Array.from(this._selectedTradeItems.values());
      if (onTradeOffer) onTradeOffer(gold, items);
    });

    this.btnTradeAccept?.addEventListener("click", () => {
      if (onTradeAccept) onTradeAccept();
    });
  }

  /**
   * Switches active HUD layout, mode selector buttons, and controls hints between
   * Game A (`village`) and Game B (`asteroid`).
   */
  setGameMode(mode) {
    const normalized = mode === "asteroid" ? "asteroid" : "village";
    this.gameMode = normalized;

    this._setElementClass(this.btnModeVillage, "active", normalized === "village");
    this._setElementClass(this.btnModeAsteroid, "active", normalized === "asteroid");

    if (this.villageHud) {
      this.villageHud.style.display = normalized === "village" ? "flex" : "none";
    }
    if (this.asteroidHud) {
      this.asteroidHud.style.display = normalized === "asteroid" ? "flex" : "none";
    }
    if (normalized === "asteroid") {
      this.hideNpcBarter();
      this.hideTarget();
    } else {
      this.hideAsteroidTarget();
    }

    if (this.tradeOfferCurrencyLabel) {
      this.tradeOfferCurrencyLabel.textContent =
        normalized === "asteroid" ? "Offer Credits:" : "Offer Gold:";
    }
    if (this.tradePartnerCurrencyLabel) {
      this.tradePartnerCurrencyLabel.textContent =
        normalized === "asteroid" ? "Credits:" : "Gold:";
    }

    if (this.controlsHintTitle) {
      this.controlsHintTitle.textContent =
        normalized === "asteroid"
          ? "6DoF Flight & Laser Controls"
          : "Village & Barter Controls";
    }

    if (this.controlsHintSubtext) {
      this.controlsHintSubtext.textContent =
        normalized === "asteroid"
          ? "Click an asteroid block to excavate ore, dock within 15m of Orbital Station (0,0,0) to bank loot, or engage rival ships within 45m."
          : "Approach a Villager or Traveler (<=5m) and press E to barter or trade.";
    }

    if (this.controlsHintGrid && typeof document.createElement === "function") {
      const hints =
        normalized === "asteroid"
          ? [
              ["WASD", "Thrust & Strafe"],
              ["Space / Shift", "Ascend / Descend"],
              ["Q/E / Arrows", "Yaw & Pitch"],
              ["Click / J", "Mine / Fire Laser"],
            ]
          : [
              ["WASD / Arrows", "Move & Turn"],
              ["Drag / Wheel", "Orbit & Zoom"],
              ["E", "Talk / Barter / Trade"],
              ["Space", "Action / Cast"],
            ];
      this._clearChildren(this.controlsHintGrid);
      for (const [keysText, labelText] of hints) {
        const item = document.createElement("div");
        item.className = "hint-item";
        const keysSpan = document.createElement("span");
        keysSpan.className = "hint-keys";
        const kbd = document.createElement("kbd");
        kbd.textContent = keysText;
        keysSpan.appendChild(kbd);
        item.appendChild(keysSpan);
        const labelNode = document.createElement("span");
        labelNode.textContent = labelText;
        item.appendChild(labelNode);
        this.controlsHintGrid.appendChild(item);
      }
    }

    if (typeof document !== "undefined") {
      if (document.body && document.body.dataset) {
        document.body.dataset.gameMode = normalized;
      }
      if (typeof document.title === "string") {
        document.title =
          normalized === "asteroid"
            ? "OpenSpec MMO - Game B: 6DoF Asteroid Miners"
            : "OpenSpec MMO - Game A: Village RPG";
      }
    }
  }

  _clearChildren(el) {
    if (!el) return;
    if (typeof el.replaceChildren === "function") {
      el.replaceChildren();
    } else {
      el.innerHTML = "";
    }
  }

  _setElementClass(el, token, enabled) {
    if (!el) return;
    if (el.classList && typeof el.classList.add === "function" && typeof el.classList.remove === "function") {
      if (enabled) el.classList.add(token);
      else el.classList.remove(token);
      return;
    }
    const classes = (el.className || "").split(/\s+/).filter(Boolean);
    const idx = classes.indexOf(token);
    if (enabled && idx === -1) classes.push(token);
    else if (!enabled && idx !== -1) classes.splice(idx, 1);
    el.className = classes.join(" ");
  }

  _renderInventorySlots(gridEl, slots) {
    if (!gridEl || !Array.isArray(slots)) return;
    this._clearChildren(gridEl);
    slots.forEach((slot) => {
      const slotEl = document.createElement("div");
      slotEl.className = `inventory-slot ${slot ? "occupied" : "empty"}`;
      if (slot) {
        const nameSpan = document.createElement("span");
        nameSpan.className = "slot-item-name";
        nameSpan.textContent = slot.name || "Item #" + slot.item_id;

        const qtySpan = document.createElement("span");
        qtySpan.className = "slot-item-qty";
        qtySpan.textContent = `x${slot.quantity || 1}`;

        const weightSpan = document.createElement("span");
        weightSpan.className = "slot-item-weight";
        weightSpan.textContent = `${((slot.unit_weight || 0) * (slot.quantity || 1)).toFixed(1)}kg`;

        slotEl.appendChild(nameSpan);
        slotEl.appendChild(qtySpan);
        slotEl.appendChild(weightSpan);
      }
      gridEl.appendChild(slotEl);
    });
  }

  updateInventory(inv, currentWeight = null) {
    if (!inv) return;
    if (Array.isArray(inv.slots)) {
      this._lastInventorySlots = inv.slots;
    }
    if (this.valGold && inv.currency !== undefined) {
      this.valGold.textContent = inv.currency;
    }
    if (this.valWeight) {
      const curVal = currentWeight != null ? currentWeight : (inv.current_weight || 0);
      const cur = Number(curVal).toFixed(1);
      const max = (inv.max_weight || 100.0).toFixed(1);
      this.valWeight.textContent = `${cur} / ${max} kg`;
    }
    this._renderInventorySlots(this.inventoryGrid, inv.slots);
  }

  /**
   * Updates the Game B safe-zone docking badge and Dock & Bank button availability.
   */
  setSafeZoneStatus(inSafeZone) {
    const safe = Boolean(inSafeZone);
    if (this.asteroidSafeZoneBadge) {
      this.asteroidSafeZoneBadge.textContent = safe ? "DOCKED SAFE ZONE" : "PVP OPEN SPACE";
      this.asteroidSafeZoneBadge.className = `safe-zone-badge ${safe ? "safe" : "danger"}`;
    }
    if (this.btnBankLoot) {
      this.btnBankLoot.disabled = !safe;
    }
  }

  /**
   * Updates the Game B Asteroid Miners HUD from an authoritative `AsteroidSectorStateResponse`.
   */
  updateAsteroidState(state) {
    if (!state) return;

    const hullHp = Number(state.hull_hp ?? 100);
    const maxHullHp = Math.max(1, Number(state.max_hull_hp ?? 100));
    const hullPct = Math.max(0, Math.min(100, (hullHp / maxHullHp) * 100));

    if (this.asteroidHullText) {
      this.asteroidHullText.textContent = `${Math.round(hullHp)} / ${Math.round(maxHullHp)} HP`;
    }
    if (this.asteroidHullBar) {
      this.asteroidHullBar.style.width = `${hullPct.toFixed(1)}%`;
      this._setElementClass(this.asteroidHullBar, "low", hullPct < 35);
    }

    const tier = Math.max(0, Math.min(255, Number(state.shininess_tier ?? 0)));
    const riskLabel =
      tier >= 180
        ? "Void Beacon"
        : tier >= 60
        ? "Golden Target"
        : tier > 0
        ? "Glinting"
        : "Stealth";
    const tierPct = Math.round((tier / 255) * 100);

    if (this.asteroidShininessText) {
      this.asteroidShininessText.textContent = `Tier ${tier} / 255 (${riskLabel})`;
    }
    if (this.asteroidShininessBar) {
      this.asteroidShininessBar.style.width = `${tierPct}%`;
    }

    if (state.in_safe_zone !== undefined) {
      this.setSafeZoneStatus(state.in_safe_zone);
    }

    const unbankedVal = Number(state.unbanked_cargo_value ?? 0);
    if (this.asteroidUnbankedValue) {
      this.asteroidUnbankedValue.textContent = String(unbankedVal);
    }

    const inv = state.inventory;
    if (inv) {
      if (Array.isArray(inv.slots)) {
        this._lastInventorySlots = inv.slots;
      }
      if (this.asteroidCargoWeight) {
        const curVal = state.current_weight != null ? state.current_weight : (inv.current_weight || 0);
        const maxVal = state.max_weight != null ? state.max_weight : (inv.max_weight || 150.0);
        this.asteroidCargoWeight.textContent = `${Number(curVal).toFixed(1)} / ${Number(maxVal).toFixed(1)} kg`;
      }
      this._renderInventorySlots(this.asteroidCargoGrid, inv.slots);
    }

    const vault = state.station_vault;
    if (vault) {
      if (this.asteroidVaultCredits) {
        this.asteroidVaultCredits.textContent = String(vault.banked_credits ?? 0);
      }
      if (this.asteroidKdStats) {
        this.asteroidKdStats.textContent = `Kills: ${vault.kills ?? 0} | Deaths: ${vault.deaths ?? 0}`;
      }
      if (this.asteroidVaultItems) {
        this._clearChildren(this.asteroidVaultItems);
        const bankedItems = Array.isArray(vault.banked_items) ? vault.banked_items : [];
        if (bankedItems.length === 0) {
          const emptyLabel = document.createElement("span");
          emptyLabel.className = "stat-label";
          emptyLabel.textContent = "No banked ore stored yet.";
          this.asteroidVaultItems.appendChild(emptyLabel);
        } else {
          for (const item of bankedItems) {
            const chip = document.createElement("span");
            chip.className = "vault-item-chip";
            chip.textContent = `${item.quantity}x ${item.name}`;
            this.asteroidVaultItems.appendChild(chip);
          }
        }
      }
    }
  }

  showAsteroidTarget(target, inSafeZone = false) {
    if (!this.asteroidTargetPanel || !target) return;
    this.asteroidTargetPanel.style.display = "block";
    const tier = (Number(target.stateFlags || 0) >>> 16) & 0xff;
    if (this.asteroidTargetName) {
      this.asteroidTargetName.textContent = `Pilot #${target.id}`;
    }
    if (this.asteroidTargetShininess) {
      this.asteroidTargetShininess.textContent = `Tier ${tier}`;
    }
    if (this.asteroidTargetDistance) {
      this.asteroidTargetDistance.textContent = `${Number(target.distance ?? 0).toFixed(1)} m`;
    }
    if (this.asteroidTargetHull) {
      this.asteroidTargetHull.textContent = `${Math.round((target.hpPct ?? 1.0) * 100)}%`;
    }
    if (this.btnFireLaser) {
      this.btnFireLaser.disabled = Boolean(inSafeZone);
    }
  }

  hideAsteroidTarget() {
    if (this.asteroidTargetPanel) {
      this.asteroidTargetPanel.style.display = "none";
    }
  }

  showTarget(name, type, distance) {
    if (!this.targetPanel) return;
    this.targetPanel.style.display = "block";
    if (this.targetName) this.targetName.textContent = name;
    if (this.targetType) this.targetType.textContent = type;
    if (this.targetDistance) this.targetDistance.textContent = `${distance.toFixed(1)} m`;
  }

  hideTarget() {
    if (this.targetPanel) {
      this.targetPanel.style.display = "none";
    }
  }

  showNpcBarter(merchant, onBarterClick) {
    if (!this.npcBarterModal) return;
    if (this.npcName) this.npcName.textContent = merchant.name || "Villager";
    if (this.npcRole) this.npcRole.textContent = merchant.role || "Merchant";
    if (this.npcGreeting) this.npcGreeting.textContent = merchant.greeting || "Greetings traveler.";

    if (this.barterRecipesList) {
      if (typeof this.barterRecipesList.replaceChildren === "function") {
        this.barterRecipesList.replaceChildren();
      } else {
        this.barterRecipesList.innerHTML = "";
      }
      const recipes = merchant.recipes || [];
      recipes.forEach((r) => {
        const card = document.createElement("div");
        card.className = "barter-recipe-card";

        const reqs = [];
        if (r.required_items) {
          r.required_items.forEach((it) => reqs.push(`${it.quantity}x ${it.name}`));
        }
        if (r.required_currency > 0) {
          reqs.push(`${r.required_currency} Gold`);
        }

        const rewards = [];
        if (r.reward_items) {
          r.reward_items.forEach((it) => rewards.push(`${it.quantity}x ${it.name}`));
        }
        if (r.reward_currency > 0) {
          rewards.push(`${r.reward_currency} Gold`);
        }

        const exchangeDiv = document.createElement("div");
        exchangeDiv.className = "recipe-exchange";

        const reqsSpan = document.createElement("span");
        reqsSpan.className = "recipe-reqs";
        reqsSpan.textContent = reqs.join(" + ") || "Free";

        const arrowSpan = document.createElement("span");
        arrowSpan.className = "barter-arrow";
        arrowSpan.textContent = "\u2192";

        const rewardsSpan = document.createElement("span");
        rewardsSpan.className = "recipe-rewards";
        rewardsSpan.textContent = rewards.join(" + ");

        exchangeDiv.appendChild(reqsSpan);
        exchangeDiv.appendChild(arrowSpan);
        exchangeDiv.appendChild(rewardsSpan);

        const btn = document.createElement("button");
        btn.className = "barter-action-btn";
        btn.setAttribute("data-recipe-id", String(r.recipe_id));
        btn.textContent = "Barter";

        btn.addEventListener("click", () => {
          if (onBarterClick) onBarterClick(r.recipe_id);
        });

        card.appendChild(exchangeDiv);
        card.appendChild(btn);

        this.barterRecipesList.appendChild(card);
      });
    }

    this.npcBarterModal.style.display = "flex";
  }

  hideNpcBarter() {
    if (this.npcBarterModal) {
      this.npcBarterModal.style.display = "none";
    }
  }

  showPlayerTrade(tradeSession) {
    if (!this.playerTradeModal) return;
    if (this.tradeSessionId) {
      this.tradeSessionId.textContent = tradeSession.session_id || "Active";
    }
    if (this.tradePartnerGold) {
      const partnerGold = tradeSession.their_offer?.currency ?? tradeSession.partner_currency ?? 0;
      this.tradePartnerGold.textContent = partnerGold;
    }

    if (this.tradeMyItems && typeof document.createElement === "function") {
      this._clearChildren(this.tradeMyItems);
      const offeredItems = Array.isArray(tradeSession.my_offer?.items) ? tradeSession.my_offer.items : [];
      const invItems = (this._lastInventorySlots || []).filter(Boolean);

      if (invItems.length > 0) {
        for (const item of invItems) {
          const row = document.createElement("div");
          row.className = "trade-item-chip";
          const label = document.createElement("span");
          label.textContent = `${item.quantity || 1}x ${item.name || "Item #" + item.item_id}`;
          const btn = document.createElement("button");
          btn.type = "button";
          const isSelected = this._selectedTradeItems.has(item.item_id);
          btn.className = `trade-item-toggle ${isSelected ? "selected" : ""}`;
          btn.textContent = isSelected ? "Offered" : "+ Offer";
          btn.addEventListener("click", () => {
            if (this._selectedTradeItems.has(item.item_id)) {
              this._selectedTradeItems.delete(item.item_id);
              btn.textContent = "+ Offer";
              this._setElementClass(btn, "selected", false);
            } else {
              this._selectedTradeItems.set(item.item_id, {
                item_id: item.item_id,
                name: item.name,
                quantity: item.quantity || 1,
                unit_weight: item.unit_weight || 0,
              });
              btn.textContent = "Offered";
              this._setElementClass(btn, "selected", true);
            }
          });
          row.appendChild(label);
          row.appendChild(btn);
          this.tradeMyItems.appendChild(row);
        }
      } else if (offeredItems.length > 0) {
        for (const item of offeredItems) {
          const row = document.createElement("div");
          row.className = "trade-item-chip";
          row.textContent = `${item.quantity || 1}x ${item.name || "Item #" + item.item_id}`;
          this.tradeMyItems.appendChild(row);
        }
      }
    }

    if (this.tradePartnerItems && typeof document.createElement === "function") {
      this._clearChildren(this.tradePartnerItems);
      const partnerItems = Array.isArray(tradeSession.their_offer?.items)
        ? tradeSession.their_offer.items
        : [];
      for (const item of partnerItems) {
        const row = document.createElement("div");
        row.className = "trade-item-chip";
        row.textContent = `${item.quantity || 1}x ${item.name || "Item #" + item.item_id}`;
        this.tradePartnerItems.appendChild(row);
      }
    }

    if (this.tradeStatusMsg) {
      let status = tradeSession.status;
      if (!status) {
        if (tradeSession.completed) {
          status = "Trade completed";
        } else if (tradeSession.my_accepted && tradeSession.their_accepted) {
          status = "Both accepted";
        } else if (tradeSession.my_locked && tradeSession.their_locked) {
          status = "Both locked";
        } else if (tradeSession.their_locked) {
          status = "Partner locked";
        } else {
          status = "Trade active";
        }
      }
      this.tradeStatusMsg.textContent = status;
    }
    this.playerTradeModal.style.display = "flex";
  }

  hidePlayerTrade() {
    this._selectedTradeItems.clear();
    if (this.playerTradeModal) {
      this.playerTradeModal.style.display = "none";
    }
  }

  setConnectionState(state, text = "", errorMessage = "", transportMode = "") {
    const normalizedState =
      state === "connected" || state === "connecting" || state === "disconnected"
        ? state
        : "disconnected";

    const defaultStatusLabel =
      normalizedState === "connected"
        ? "Connected"
        : normalizedState === "connecting"
        ? "Connecting..."
        : "Disconnected";
    const resolvedStatusText =
      typeof text === "string" && text.trim().length > 0 ? text.trim() : defaultStatusLabel;

    if (this.statusDot) {
      this.statusDot.className = `status-indicator ${normalizedState}`;
      this.statusDot.title = resolvedStatusText;
    }

    if (this.statusText) {
      this.statusText.textContent = resolvedStatusText;
      this.statusText.className = `status-badge ${normalizedState}`;
    }

    if (this.transportBadge) {
      const normalizedMode =
        typeof transportMode === "string" ? transportMode.trim().toLowerCase() : "";

      if (
        normalizedMode === "webtransport" ||
        normalizedMode === "http3" ||
        normalizedMode === "http/3"
      ) {
        this._lastTransportMode = "webtransport";
      } else if (
        normalizedMode === "websocket" ||
        normalizedMode === "ws" ||
        normalizedMode === "wss"
      ) {
        this._lastTransportMode = "websocket";
      } else if (normalizedState !== "connected") {
        this._lastTransportMode = null;
      }

      if (normalizedState === "disconnected") {
        this._lastTransportMode = null;
        this.transportBadge.textContent = "Offline";
        this.transportBadge.className = "transport-badge offline";
      } else if (this._lastTransportMode === "webtransport") {
        this.transportBadge.textContent = "HTTP/3";
        this.transportBadge.className = "transport-badge webtransport";
      } else if (this._lastTransportMode === "websocket") {
        this.transportBadge.textContent = "WebSocket";
        this.transportBadge.className = "transport-badge websocket";
      } else if (normalizedState === "connecting") {
        this.transportBadge.textContent = "Negotiating";
        this.transportBadge.className = "transport-badge connecting";
      } else {
        this.transportBadge.textContent = "Online";
        this.transportBadge.className = "transport-badge online";
      }
    }

    if (normalizedState === "connected") {
      if (this.btnConnect) this.btnConnect.disabled = true;
      if (this.btnDisconnect) this.btnDisconnect.disabled = false;
    } else if (normalizedState === "disconnected") {
      if (this.btnConnect) this.btnConnect.disabled = false;
      if (this.btnDisconnect) this.btnDisconnect.disabled = true;
    } else if (normalizedState === "connecting") {
      if (this.btnConnect) this.btnConnect.disabled = true;
      if (this.btnDisconnect) this.btnDisconnect.disabled = false;
    }

    if (this.connectionError) {
      const normalizedError = typeof errorMessage === "string" ? errorMessage.trim() : "";
      if (
        normalizedState !== "connected" &&
        normalizedState !== "connecting" &&
        normalizedError.length > 0
      ) {
        this.connectionError.textContent = normalizedError;
        this._setElementHidden(this.connectionError, false);
      } else {
        this.connectionError.textContent = "";
        this._setElementHidden(this.connectionError, true);
      }
    }
  }

  /**
   * Displays a timed, non-intrusive toast notification in #hud-toast.
   */
  showToast(message, variant = "info", durationMs = 3000) {
    if (!this.hudToast) return;

    if (this._toastTimer !== null) {
      clearTimeout(this._toastTimer);
      this._toastTimer = null;
    }

    const text = typeof message === "string" ? message.trim() : String(message ?? "").trim();
    if (!text) {
      this._setElementHidden(this.hudToast, true);
      return;
    }

    const allowedVariants = new Set(["info", "success", "warning", "error"]);
    const normalizedVariant =
      typeof variant === "string" ? variant.trim().toLowerCase() : "info";
    const safeVariant = allowedVariants.has(normalizedVariant) ? normalizedVariant : "info";

    this.hudToast.textContent = text;
    this.hudToast.className = `hud-toast toast-${safeVariant}`;
    this._setElementHidden(this.hudToast, false);

    if (typeof durationMs === "number" && Number.isFinite(durationMs) && durationMs > 0) {
      this._toastTimer = setTimeout(() => {
        this._toastTimer = null;
        this.hideToast();
      }, durationMs);
    }
  }

  /**
   * Hides the #hud-toast notification banner and clears any pending dismiss timer.
   */
  hideToast() {
    if (this._toastTimer !== null) {
      clearTimeout(this._toastTimer);
      this._toastTimer = null;
    }
    if (this.hudToast) {
      this._setElementHidden(this.hudToast, true);
    }
  }

  _setElementHidden(el, hidden) {
    if (el.classList) {
      if (hidden) {
        el.classList.add("hidden");
      } else {
        el.classList.remove("hidden");
      }
      return;
    }
    const classes = (el.className || "").split(/\s+/).filter(Boolean);
    const hasHidden = classes.includes("hidden");
    if (hidden && !hasHidden) {
      classes.push("hidden");
    } else if (!hidden && hasHidden) {
      const idx = classes.indexOf("hidden");
      if (idx !== -1) classes.splice(idx, 1);
    }
    el.className = classes.join(" ");
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
      if (this.fpsCounter) {
        this.fpsCounter.textContent = `${fps} FPS`;
      }
      this.frameCount = 0;
      this.lastFpsUpdate = now;
    }
  }
}
