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

    // Village HUD & Modals
    this.valGold = document.getElementById("val-gold");
    this.valWeight = document.getElementById("val-weight");
    this.inventoryGrid = document.getElementById("inventory-grid");

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
    this.tradeOfferGold = document.getElementById("trade-offer-gold");
    this.tradeMyItems = document.getElementById("trade-my-items");
    this.tradePartnerGold = document.getElementById("trade-partner-gold");
    this.tradePartnerItems = document.getElementById("trade-partner-items");
    this.tradeStatusMsg = document.getElementById("trade-status-msg");
    this.btnTradeOffer = document.getElementById("btn-trade-offer");
    this.btnTradeAccept = document.getElementById("btn-trade-accept");
    this.btnTradeCancel = document.getElementById("btn-trade-cancel");
    this.btnCloseTrade = document.getElementById("btn-close-trade");

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
  }) {
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
      if (onTradeOffer) onTradeOffer(gold, []);
    });

    this.btnTradeAccept?.addEventListener("click", () => {
      if (onTradeAccept) onTradeAccept();
    });
  }

  updateInventory(inv) {
    if (!inv) return;
    if (this.valGold && inv.currency !== undefined) {
      this.valGold.textContent = inv.currency;
    }
    if (this.valWeight) {
      const cur = (inv.current_weight || 0).toFixed(1);
      const max = (inv.max_weight || 100.0).toFixed(1);
      this.valWeight.textContent = `${cur} / ${max} kg`;
    }
    if (this.inventoryGrid && Array.isArray(inv.slots)) {
      this.inventoryGrid.innerHTML = "";
      inv.slots.forEach((slot, idx) => {
        const slotEl = document.createElement("div");
        slotEl.className = `inventory-slot ${slot ? "occupied" : "empty"}`;
        if (slot) {
          slotEl.innerHTML = `
            <span class="slot-item-name">${slot.name || "Item #" + slot.item_id}</span>
            <span class="slot-item-qty">x${slot.quantity || 1}</span>
            <span class="slot-item-weight">${((slot.unit_weight || 0) * (slot.quantity || 1)).toFixed(1)}kg</span>
          `;
        }
        this.inventoryGrid.appendChild(slotEl);
      });
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
      this.barterRecipesList.innerHTML = "";
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

        card.innerHTML = `
          <div class="recipe-exchange">
            <span class="recipe-reqs">${reqs.join(" + ") || "Free"}</span>
            <span class="barter-arrow">&rarr;</span>
            <span class="recipe-rewards">${rewards.join(" + ")}</span>
          </div>
          <button class="barter-action-btn" data-recipe-id="${r.recipe_id}">Barter</button>
        `;

        const btn = card.querySelector(".barter-action-btn");
        btn.addEventListener("click", () => {
          if (onBarterClick) onBarterClick(r.recipe_id);
        });

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
      this.tradePartnerGold.textContent = tradeSession.partner_currency || 0;
    }
    if (this.tradeStatusMsg) {
      this.tradeStatusMsg.textContent = tradeSession.status || "Trade active";
    }
    this.playerTradeModal.style.display = "flex";
  }

  hidePlayerTrade() {
    if (this.playerTradeModal) {
      this.playerTradeModal.style.display = "none";
    }
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
