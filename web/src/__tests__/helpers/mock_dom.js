import { vi } from "vitest";

function matchesSimpleSelector(node, selector) {
  if (!node || !selector) return false;
  let rest = selector.trim();
  if (!rest) return false;

  // Attribute selector suffix: [attr] or [attr="val"] / [attr='val']
  const attrMatches = [];
  rest = rest.replace(/\[([^\]=]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\]]*)))?\]/g, (_m, k, v1, v2, v3) => {
    attrMatches.push({ key: k.trim(), value: v1 ?? v2 ?? v3 ?? null });
    return "";
  });

  // Extract #id and .class tokens and optional leading tagName
  let expectedId = null;
  const expectedClasses = [];
  rest = rest.replace(/#([A-Za-z0-9_-]+)/g, (_m, id) => {
    expectedId = id;
    return "";
  });
  rest = rest.replace(/\.([A-Za-z0-9_-]+)/g, (_m, cls) => {
    expectedClasses.push(cls);
    return "";
  });
  const expectedTag = rest.trim().toLowerCase();

  if (expectedTag && (node.tagName || "").toLowerCase() !== expectedTag) {
    return false;
  }
  if (expectedId !== null && node.id !== expectedId) {
    return false;
  }
  if (expectedClasses.length > 0) {
    const nodeClasses = (node.className || "").split(/\s+/).filter(Boolean);
    for (const cls of expectedClasses) {
      if (!nodeClasses.includes(cls)) return false;
    }
  }
  for (const attr of attrMatches) {
    const actual = node.getAttribute ? node.getAttribute(attr.key) : null;
    if (actual === null || actual === undefined) return false;
    if (attr.value !== null && String(actual) !== attr.value) return false;
  }
  return true;
}

function createEventTargetMixin(target) {
  target._listeners = new Map();
  target.addEventListener = (type, listener) => {
    if (typeof listener !== "function") return;
    if (!target._listeners.has(type)) {
      target._listeners.set(type, new Set());
    }
    target._listeners.get(type).add(listener);
  };
  target.removeEventListener = (type, listener) => {
    target._listeners.get(type)?.delete(listener);
  };
  target.dispatchEvent = (event) => {
    const ev = event && typeof event === "object" ? event : { type: String(event) };
    if (!ev.target) ev.target = target;
    const handlers = target._listeners.get(ev.type);
    if (!handlers) return true;
    for (const fn of Array.from(handlers)) {
      fn(ev);
    }
    return true;
  };
  return target;
}

/**
 * Creates a lightweight DOM element mock supporting classList, attributes,
 * event listeners, XSS innerHTML tracking, and CSS selector queries.
 */
export function createMockElement(id = "", className = "", tagName = "div") {
  const el = {
    id,
    tagName: tagName.toUpperCase(),
    className,
    style: {},
    children: [],
    parentNode: null,
    textContent: "",
    title: "",
    value: "",
    type: "",
    disabled: false,
    checked: false,
    dataset: {},
    attributes: {},
    _innerHTML: "",
    get innerHTML() {
      if (this._innerHTML) return this._innerHTML;
      return this.children
        .map((c) => c.textContent || c.innerHTML || "")
        .join("");
    },
    set innerHTML(val) {
      this._innerHTML = String(val ?? "");
      this.children = [];
    },
    setAttribute(name, val) {
      this.attributes[name] = String(val);
      if (name === "id") this.id = String(val);
      if (name === "class") this.className = String(val);
    },
    getAttribute(name) {
      if (name === "id" && this.id) return this.id;
      if (name === "class") return this.className;
      return Object.prototype.hasOwnProperty.call(this.attributes, name)
        ? this.attributes[name]
        : null;
    },
    hasAttribute(name) {
      return this.getAttribute(name) !== null;
    },
    removeAttribute(name) {
      delete this.attributes[name];
    },
    appendChild(child) {
      if (!child) return child;
      child.parentNode = this;
      this.children.push(child);
      return child;
    },
    removeChild(child) {
      const idx = this.children.indexOf(child);
      if (idx !== -1) {
        this.children.splice(idx, 1);
        child.parentNode = null;
      }
      return child;
    },
    replaceChildren(...newChildren) {
      for (const oldChild of this.children) {
        if (oldChild) oldChild.parentNode = null;
      }
      this.children = [];
      for (const child of newChildren) {
        this.appendChild(child);
      }
    },
    remove() {
      this.parentNode?.removeChild?.(this);
    },
    click() {
      this.dispatchEvent({ type: "click", target: this });
    },
    querySelectorAll(selector) {
      const results = [];
      const walk = (node) => {
        for (const child of node.children || []) {
          if (matchesSimpleSelector(child, selector)) {
            results.push(child);
          }
          walk(child);
        }
      };
      walk(this);
      return results;
    },
    querySelector(selector) {
      return this.querySelectorAll(selector)[0] ?? null;
    },
  };

  el.classList = {
    add(...tokens) {
      const current = new Set((el.className || "").split(/\s+/).filter(Boolean));
      tokens.forEach((t) => {
        if (t) current.add(t);
      });
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
    toggle(token, force) {
      const present = this.contains(token);
      const enable = force !== undefined ? Boolean(force) : !present;
      if (enable) this.add(token);
      else this.remove(token);
      return enable;
    },
  };

  return createEventTargetMixin(el);
}

/**
 * Installs a mock `globalThis.document` with auto-created or pre-seeded elements.
 */
export function installMockDocument({ strict = false, initialIds = [] } = {}) {
  const elements = new Map();
  for (const id of initialIds) {
    elements.set(id, createMockElement(id));
  }

  const body = createMockElement("", "", "body");
  const doc = createEventTargetMixin({
    title: "",
    body,
    getElementById(id) {
      if (elements.has(id)) return elements.get(id);
      if (strict) return null;
      const el = createMockElement(id);
      elements.set(id, el);
      return el;
    },
    createElement(tagName = "div") {
      return createMockElement("", "", tagName);
    },
    querySelectorAll(selector) {
      const matches = [];
      for (const el of elements.values()) {
        if (matchesSimpleSelector(el, selector)) matches.push(el);
      }
      return matches;
    },
    querySelector(selector) {
      return this.querySelectorAll(selector)[0] ?? null;
    },
  });

  vi.stubGlobal("document", doc);

  return {
    document: doc,
    elements,
    getElement(id) {
      return doc.getElementById(id);
    },
  };
}

/**
 * Installs a mock `globalThis.window` and `globalThis.sessionStorage` for routing & token tests.
 */
export function installMockWindow({
  search = "",
  pathname = "/",
  hostname = "127.0.0.1",
  host = hostname,
  protocol = "http:",
} = {}) {
  const storageMap = new Map();
  const sessionStorage = {
    getItem: (key) => (storageMap.has(key) ? storageMap.get(key) : null),
    setItem: (key, val) => storageMap.set(key, String(val)),
    removeItem: (key) => storageMap.delete(key),
    clear: () => storageMap.clear(),
  };

  const location = {
    search,
    pathname,
    hostname,
    host,
    protocol,
    href: `${protocol}//${host}${pathname}${search}`,
  };

  const win = createEventTargetMixin({
    location,
    sessionStorage,
    history: {
      replaceState: vi.fn((_state, _title, nextUrl) => {
        if (typeof nextUrl === "string") {
          const parsed = new URL(nextUrl, `${location.protocol}//${location.host}`);
          location.pathname = parsed.pathname;
          location.search = parsed.search;
          location.href = parsed.toString();
        }
      }),
    },
  });

  vi.stubGlobal("sessionStorage", sessionStorage);
  vi.stubGlobal("window", win);

  return { window: win, sessionStorage, storageMap, location };
}

export function createMerchantFixture(overrides = {}) {
  return {
    name: "Blacksmith Ilmari",
    role: "Master Artisan",
    greeting: "Fine steel and tools forged for hardy travelers.",
    recipes: [
      {
        recipe_id: 101,
        required_items: [
          { item_id: 1, name: "Iron Ore", quantity: 3 },
          { item_id: 2, name: "Oak Timber", quantity: 1 },
        ],
        required_currency: 15,
        reward_items: [{ item_id: 10, name: "Steel Pickaxe", quantity: 1 }],
        reward_currency: 0,
      },
      {
        recipe_id: 102,
        required_items: [{ item_id: 3, name: "Wolf Pelt", quantity: 2 }],
        required_currency: 0,
        reward_items: [],
        reward_currency: 35,
      },
    ],
    ...overrides,
  };
}

export function createTradeSessionFixture(overrides = {}) {
  return {
    session_id: 42,
    my_offer: { currency: 0, items: [] },
    their_offer: { currency: 0, items: [] },
    my_locked: false,
    their_locked: false,
    my_accepted: false,
    their_accepted: false,
    completed: false,
    ...overrides,
  };
}
