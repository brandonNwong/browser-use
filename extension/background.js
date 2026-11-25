"use strict";

const EXTENSION_VERSION = "3.0.0";
const PROTOCOL_VERSION = "3.0.0";
const DEFAULT_HUB_URL = "ws://127.0.0.1:19455/";
const STORAGE_KEY_SECRET = "orange_bridge.secret";
const STORAGE_KEY_URL = "orange_bridge.url";
const MAX_CAPTURED_REQUESTS = 500;

let config = {
  secret: "",
  url: DEFAULT_HUB_URL
};

let socket = null;
let connectionState = "starting"; // unconfigured | connecting | connected | unauthorized | disconnected
let lastError = "";
let reconnectTimer = null;
let reconnectDelayMs = 500;
let boundTabId = null;
let connectWaiters = [];

// Network tracking & route interception
const inFlightRequests = new Map();
const capturedRequests = [];
const activeRoutes = [];
const pendingHeaderOverrides = new Map();

// ── Helpers ───────────────────────────────────────────────────────────────

function extractSecret(raw) {
  if (raw == null) return "";
  let text = String(raw).trim();
  if (!text) return "";

  const cmdMatch = text.match(/OrangeBridge\.setSecret\s*\(\s*(["'`])([\s\S]+?)\1\s*\)/);
  if (cmdMatch) {
    const quote = cmdMatch[1];
    const inner = cmdMatch[2];
    if (quote === '"') {
      try {
        return JSON.parse(`"${inner}"`).trim();
      } catch (_) {
        return inner.trim();
      }
    }
    return inner.trim();
  }

  if (
    (text.startsWith('"') && text.endsWith('"')) ||
    (text.startsWith("'") && text.endsWith("'"))
  ) {
    text = text.slice(1, -1).trim();
  }
  return text;
}

function normalizeHubUrl(raw) {
  if (raw == null || String(raw).trim() === "") {
    return DEFAULT_HUB_URL;
  }
  let text = String(raw).trim();
  if (/^\d{1,5}$/.test(text)) {
    const port = Number(text);
    if (port < 1 || port > 65535) {
      throw new Error(`Port must be between 1 and 65535 (got ${text})`);
    }
    return `ws://127.0.0.1:${port}/`;
  }
  if (/^http:\/\//i.test(text)) {
    text = "ws://" + text.slice(7);
  } else if (/^https:\/\//i.test(text)) {
    text = "wss://" + text.slice(8);
  } else if (!/^wss?:\/\//i.test(text)) {
    text = "ws://" + text;
  }

  let parsed;
  try {
    parsed = new URL(text);
  } catch (err) {
    throw new Error(`Invalid hub URL: ${text}`);
  }
  if (parsed.protocol !== "ws:" && parsed.protocol !== "wss:") {
    throw new Error(`Hub URL must use ws:// or wss:// (got ${parsed.protocol})`);
  }
  const host = parsed.hostname.toLowerCase();
  if (host !== "127.0.0.1" && host !== "localhost" && host !== "[::1]" && host !== "::1") {
    throw new Error(`Refusing non-loopback hub host: ${parsed.hostname}`);
  }
  const port = parsed.port || "19455";
  return `${parsed.protocol}//${parsed.hostname}:${port}/`;
}

function updateBadge() {
  if (!browser.browserAction) return;
  try {
    if (connectionState === "connected") {
      browser.browserAction.setBadgeText({ text: "ON" });
      browser.browserAction.setBadgeBackgroundColor({ color: "#16A34A" });
    } else if (connectionState === "unauthorized") {
      browser.browserAction.setBadgeText({ text: "!" });
      browser.browserAction.setBadgeBackgroundColor({ color: "#DC2626" });
    } else if (connectionState === "unconfigured") {
      browser.browserAction.setBadgeText({ text: "?" });
      browser.browserAction.setBadgeBackgroundColor({ color: "#FF6B1A" });
    } else {
      browser.browserAction.setBadgeText({ text: "" });
    }
  } catch (_) {
    // Ignore badge errors on environments without browserAction support.
  }
}

function resolveConnectWaiters(result) {
  const waiters = connectWaiters.splice(0, connectWaiters.length);
  for (const resolve of waiters) {
    try {
      resolve(result);
    } catch (_) {}
  }
}

function waitForConnectAttempt(timeoutMs = 2500) {
  return new Promise((resolve) => {
    if (connectionState === "connected") {
      resolve({ ok: true, connected: true, url: config.url, error: "" });
      return;
    }
    if (!config.secret) {
      resolve({ ok: false, connected: false, url: config.url, error: "No secret configured" });
      return;
    }
    const timer = setTimeout(() => {
      const idx = connectWaiters.indexOf(done);
      if (idx >= 0) connectWaiters.splice(idx, 1);
      resolve({
        ok: connectionState !== "unauthorized",
        connected: connectionState === "connected",
        url: config.url,
        error: lastError || "Hub did not respond yet"
      });
    }, timeoutMs);

    function done(result) {
      clearTimeout(timer);
      resolve(result);
    }
    connectWaiters.push(done);
  });
}

function getStatusSnapshot() {
  return {
    connected: connectionState === "connected",
    state: connectionState,
    url: config.url,
    hasSecret: Boolean(config.secret),
    lastError,
    boundTabId,
    routes: activeRoutes.map((r) => r.urlPattern),
    version: EXTENSION_VERSION,
    protocol: PROTOCOL_VERSION
  };
}

// ── WebSocket lifecycle ───────────────────────────────────────────────────

function clearReconnectTimer() {
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

function scheduleReconnect(delayMs) {
  clearReconnectTimer();
  if (!config.secret) return;
  const wait = delayMs != null ? delayMs : reconnectDelayMs;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect();
  }, wait);
  reconnectDelayMs = Math.min(reconnectDelayMs * 2, 5000);
}

function disconnectSocket() {
  clearReconnectTimer();
  if (socket) {
    const old = socket;
    socket = null;
    try {
      old.onopen = null;
      old.onmessage = null;
      old.onerror = null;
      old.onclose = null;
      old.close(1000, "Reconnecting");
    } catch (_) {}
  }
}

function sendFrame(payload) {
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    return false;
  }
  try {
    socket.send(JSON.stringify(payload));
    return true;
  } catch (_) {
    return false;
  }
}

function connect() {
  disconnectSocket();

  if (!config.secret) {
    connectionState = "unconfigured";
    lastError = "";
    updateBadge();
    return;
  }

  connectionState = "connecting";
  updateBadge();

  let ws;
  try {
    ws = new WebSocket(config.url);
  } catch (err) {
    connectionState = "disconnected";
    lastError = err && err.message ? err.message : String(err);
    updateBadge();
    resolveConnectWaiters({ ok: false, connected: false, url: config.url, error: lastError });
    scheduleReconnect();
    return;
  }

  socket = ws;

  ws.onopen = () => {
    if (socket !== ws) return;
    sendFrame({
      type: "hello",
      secret: config.secret,
      client: "gecko-orange",
      version: EXTENSION_VERSION,
      protocol: PROTOCOL_VERSION,
      userAgent: navigator.userAgent
    });
  };

  ws.onmessage = (event) => {
    if (socket !== ws) return;
    let msg;
    try {
      msg = JSON.parse(String(event.data));
    } catch (_) {
      return;
    }
    if (!msg || typeof msg !== "object") return;

    if (msg.type === "hello_ok") {
      connectionState = "connected";
      lastError = "";
      reconnectDelayMs = 500;
      updateBadge();
      resolveConnectWaiters({ ok: true, connected: true, url: config.url, error: "" });
      return;
    }

    if (msg.type === "ping") {
      sendFrame({ type: "pong", ts: Date.now() });
      return;
    }

    if (msg.type === "cmd") {
      dispatchCommand(msg);
    }
  };

  ws.onerror = () => {
    if (socket !== ws) return;
    if (connectionState !== "unauthorized") {
      lastError = `Unable to reach hub at ${config.url}`;
    }
  };

  ws.onclose = (event) => {
    if (socket !== ws) return;
    socket = null;

    if (event.code === 4001) {
      connectionState = "unauthorized";
      lastError = "Hub rejected the shared secret (4001 Unauthorized)";
      updateBadge();
      resolveConnectWaiters({ ok: false, connected: false, url: config.url, error: lastError });
      scheduleReconnect(10000);
      return;
    }

    if (event.code === 4002) {
      connectionState = "disconnected";
      lastError = "Replaced by another Gecko Orange connection";
      updateBadge();
      scheduleReconnect(5000);
      return;
    }

    connectionState = "disconnected";
    if (!lastError) {
      lastError = event.reason || `Disconnected (code ${event.code})`;
    }
    updateBadge();
    resolveConnectWaiters({ ok: true, connected: false, url: config.url, error: lastError });
    scheduleReconnect();
  };
}

// ── Public OrangeBridge Console API ───────────────────────────────────────

const OrangeBridge = {
  async setSecret(secret) {
    const cleaned = extractSecret(secret);
    if (!cleaned) {
      throw new Error("Secret must be a non-empty string.");
    }
    config.secret = cleaned;
    await browser.storage.local.set({ [STORAGE_KEY_SECRET]: cleaned });
    reconnectDelayMs = 500;
    lastError = "";
    const wait = waitForConnectAttempt(2500);
    connect();
    const res = await wait;
    console.info("[Gecko Orange] setSecret:", res.connected ? `connected to ${res.url}` : res.error || "waiting for hub");
    return res;
  },

  async setUrl(url) {
    const normalized = normalizeHubUrl(url);
    config.url = normalized;
    await browser.storage.local.set({ [STORAGE_KEY_URL]: normalized });
    reconnectDelayMs = 500;
    lastError = "";
    const wait = waitForConnectAttempt(2500);
    connect();
    const res = await wait;
    console.info("[Gecko Orange] setUrl:", normalized, res.connected ? "(connected)" : `(${res.error || "waiting"})`);
    return res;
  },

  async clearSecret() {
    config.secret = "";
    await browser.storage.local.remove(STORAGE_KEY_SECRET);
    disconnectSocket();
    connectionState = "unconfigured";
    lastError = "";
    updateBadge();
    return getStatusSnapshot();
  },

  async reconnect() {
    reconnectDelayMs = 500;
    lastError = "";
    const wait = waitForConnectAttempt(2500);
    connect();
    return await wait;
  },

  status() {
    return getStatusSnapshot();
  }
};

globalThis.OrangeBridge = OrangeBridge;

// ── Popup Messaging ───────────────────────────────────────────────────────

browser.runtime.onMessage.addListener((message) => {
  if (!message || typeof message !== "object") return undefined;

  if (message.type === "orange_popup_get_status") {
    return Promise.resolve(getStatusSnapshot());
  }

  if (message.type === "orange_popup_save") {
    return (async () => {
      try {
        if (message.url && String(message.url).trim()) {
          config.url = normalizeHubUrl(message.url);
          await browser.storage.local.set({ [STORAGE_KEY_URL]: config.url });
        }
        const cleanedSecret = extractSecret(message.secret);
        if (cleanedSecret) {
          config.secret = cleanedSecret;
          await browser.storage.local.set({ [STORAGE_KEY_SECRET]: cleanedSecret });
        }
        if (!config.secret) {
          return {
            ok: false,
            connected: false,
            error: "Please paste a shared secret or Zen setup command.",
            status: getStatusSnapshot()
          };
        }
        reconnectDelayMs = 500;
        lastError = "";
        const wait = waitForConnectAttempt(2500);
        connect();
        const res = await wait;
        return {
          ok: true,
          connected: res.connected,
          error: res.connected ? "" : res.error,
          status: getStatusSnapshot()
        };
      } catch (err) {
        return {
          ok: false,
          connected: false,
          error: err && err.message ? err.message : String(err),
          status: getStatusSnapshot()
        };
      }
    })();
  }

  if (message.type === "orange_popup_reconnect") {
    return (async () => {
      const res = await OrangeBridge.reconnect();
      return {
        ok: res.ok,
        connected: res.connected,
        error: res.connected ? "" : res.error,
        status: getStatusSnapshot()
      };
    })();
  }

  if (message.type === "orange_popup_clear_secret") {
    return OrangeBridge.clearSecret().then((status) => ({ ok: true, status }));
  }

  return undefined;
});

// ── Tab Tracking & Helpers ────────────────────────────────────────────────

browser.tabs.onRemoved.addListener((tabId) => {
  if (boundTabId === tabId) {
    boundTabId = null;
  }
});

async function resolveTargetTab(requestedTabId) {
  if (requestedTabId != null) {
    try {
      return await browser.tabs.get(Number(requestedTabId));
    } catch (_) {
      throw new Error(`Tab ${requestedTabId} not found`);
    }
  }

  if (boundTabId != null) {
    try {
      return await browser.tabs.get(boundTabId);
    } catch (_) {
      boundTabId = null;
    }
  }

  const lastFocused = await browser.tabs.query({ active: true, lastFocusedWindow: true });
  if (lastFocused.length > 0) return lastFocused[0];

  const anyActive = await browser.tabs.query({ active: true });
  if (anyActive.length > 0) return anyActive[0];

  const allTabs = await browser.tabs.query({});
  if (allTabs.length > 0) return allTabs[0];

  throw new Error("No open browser tabs found");
}

function serializeTab(tab) {
  return {
    id: tab.id,
    windowId: tab.windowId,
    index: tab.index,
    active: Boolean(tab.active),
    pinned: Boolean(tab.pinned),
    status: tab.status || "complete",
    title: tab.title || "",
    url: tab.url || ""
  };
}

function isScriptableUrl(url) {
  if (!url) return false;
  return /^(https?|file):/i.test(url);
}

async function ensureContentScript(tab) {
  if (!isScriptableUrl(tab.url)) {
    throw new Error(`Cannot interact with restricted page (${tab.url || "empty URL"}). Navigate to an http(s) page first.`);
  }
  try {
    const pong = await browser.tabs.sendMessage(tab.id, { __geckoOrangePing: true });
    if (pong && pong.ok) return;
  } catch (_) {
    // Content script not yet injected in this tab.
  }
  await browser.tabs.executeScript(tab.id, { file: "content.js" });
}

async function sendToPage(tab, cmd) {
  await ensureContentScript(tab);
  const reply = await browser.tabs.sendMessage(tab.id, {
    __geckoOrange: true,
    tabId: tab.id,
    cmd
  });
  if (!reply || typeof reply !== "object") {
    throw new Error(`No response from page for action '${cmd.action}'`);
  }
  if (reply.ok === false) {
    throw new Error(reply.error || `Page action '${cmd.action}' failed`);
  }
  return reply;
}

function waitForTabComplete(tabId, timeoutMs = 30000, requireLoadingFirst = false) {
  return new Promise((resolve) => {
    let seenLoading = !requireLoadingFirst;
    let settled = false;

    const cleanup = () => {
      clearTimeout(timer);
      browser.tabs.onUpdated.removeListener(onUpdated);
      browser.tabs.onRemoved.removeListener(onRemoved);
    };

    const finish = async () => {
      if (settled) return;
      settled = true;
      cleanup();
      try {
        const t = await browser.tabs.get(tabId);
        resolve(t);
      } catch (_) {
        resolve({ id: tabId, url: "", title: "", status: "complete" });
      }
    };

    const onUpdated = (updatedId, changeInfo, tab) => {
      if (updatedId !== tabId) return;
      if (changeInfo.status === "loading") {
        seenLoading = true;
      }
      if (seenLoading && (changeInfo.status === "complete" || tab.status === "complete")) {
        finish();
      }
    };

    const onRemoved = (removedId) => {
      if (removedId === tabId) finish();
    };

    const timer = setTimeout(finish, timeoutMs);
    browser.tabs.onUpdated.addListener(onUpdated);
    browser.tabs.onRemoved.addListener(onRemoved);

    if (!requireLoadingFirst) {
      browser.tabs.get(tabId).then((t) => {
        if (!settled && t && t.status === "complete") {
          // Give navigation a brief tick to transition into loading before settling.
          setTimeout(() => {
            if (!settled) {
              browser.tabs.get(tabId).then((t2) => {
                if (!settled && t2 && t2.status === "complete") {
                  finish();
                }
              }).catch(() => {});
            }
          }, 150);
        }
      }).catch(() => {});
    }
  });
}

// ── Network Capture & Routing ─────────────────────────────────────────────

function compileUrlPattern(pattern) {
  const text = String(pattern || "");
  if (text.length >= 2 && text.startsWith("/")) {
    const lastSlash = text.lastIndexOf("/");
    if (lastSlash > 0) {
      const body = text.slice(1, lastSlash);
      const flags = text.slice(lastSlash + 1);
      try {
        const regex = new RegExp(body, flags);
        return (url) => regex.test(url);
      } catch (_) {
        // Fall through to literal substring.
      }
    }
  }
  if (text.includes("*")) {
    const escaped = text
      .replace(/[.+?^${}()|[\]\\]/g, "\\$&")
      .replace(/\*\*/g, ".*")
      .replace(/\*/g, "[^/]*");
    try {
      const globRegex = new RegExp(escaped);
      return (url) => url.includes(text) || globRegex.test(url);
    } catch (_) {}
  }
  return (url) => url.includes(text);
}

function recordRequest(entry) {
  capturedRequests.push(entry);
  if (capturedRequests.length > MAX_CAPTURED_REQUESTS) {
    capturedRequests.splice(0, capturedRequests.length - MAX_CAPTURED_REQUESTS);
  }
}

browser.webRequest.onBeforeRequest.addListener(
  (details) => {
    inFlightRequests.set(details.requestId, {
      id: details.requestId,
      tabId: details.tabId,
      method: details.method || "GET",
      url: details.url,
      startTime: details.timeStamp || Date.now(),
      status: null,
      timeMs: null,
      error: null
    });

    for (let i = activeRoutes.length - 1; i >= 0; i--) {
      const route = activeRoutes[i];
      if (route.tabId != null && details.tabId !== route.tabId) continue;
      if (!route.matcher(details.url)) continue;

      if (route.abort) {
        return { cancel: true };
      }
      if (route.redirectUrl) {
        return { redirectUrl: route.redirectUrl };
      }
      if (route.body !== undefined || route.status != null || route.headers || route.contentType) {
        pendingHeaderOverrides.set(details.requestId, route);
        if (route.body !== undefined && typeof browser.webRequest.filterResponseData === "function") {
          try {
            const filter = browser.webRequest.filterResponseData(details.requestId);
            const encoder = new TextEncoder();
            const textBody = typeof route.body === "string"
              ? route.body
              : JSON.stringify(route.body);
            filter.onstart = () => {
              filter.write(encoder.encode(textBody));
              filter.close();
            };
            filter.ondata = () => {};
          } catch (_) {
            // Ignore if filterResponseData is unavailable for this request.
          }
        }
        return {};
      }
    }
    return {};
  },
  { urls: ["<all_urls>"] },
  ["blocking"]
);

browser.webRequest.onHeadersReceived.addListener(
  (details) => {
    const route = pendingHeaderOverrides.get(details.requestId);
    if (!route) return {};
    pendingHeaderOverrides.delete(details.requestId);

    const responseHeaders = (details.responseHeaders || []).slice();
    const setHeader = (name, value) => {
      const lower = name.toLowerCase();
      const idx = responseHeaders.findIndex((h) => h.name.toLowerCase() === lower);
      if (idx >= 0) {
        responseHeaders[idx] = { name, value: String(value) };
      } else {
        responseHeaders.push({ name, value: String(value) });
      }
    };

    if (route.contentType) {
      setHeader("Content-Type", route.contentType);
    }
    if (route.headers && typeof route.headers === "object") {
      for (const [k, v] of Object.entries(route.headers)) {
        setHeader(k, v);
      }
    }

    const result = { responseHeaders };
    if (route.status != null) {
      result.statusLine = `HTTP/1.1 ${route.status} Mocked`;
    }
    return result;
  },
  { urls: ["<all_urls>"] },
  ["blocking", "responseHeaders"]
);

browser.webRequest.onCompleted.addListener(
  (details) => {
    pendingHeaderOverrides.delete(details.requestId);
    const tracked = inFlightRequests.get(details.requestId);
    inFlightRequests.delete(details.requestId);
    const endTime = details.timeStamp || Date.now();
    const startTime = tracked ? tracked.startTime : endTime;
    recordRequest({
      tabId: details.tabId,
      method: details.method || (tracked && tracked.method) || "GET",
      url: details.url,
      status: details.statusCode,
      timeMs: Math.max(0, Math.round(endTime - startTime)),
      error: null
    });
  },
  { urls: ["<all_urls>"] }
);

browser.webRequest.onErrorOccurred.addListener(
  (details) => {
    pendingHeaderOverrides.delete(details.requestId);
    const tracked = inFlightRequests.get(details.requestId);
    inFlightRequests.delete(details.requestId);
    const endTime = details.timeStamp || Date.now();
    const startTime = tracked ? tracked.startTime : endTime;
    recordRequest({
      tabId: details.tabId,
      method: details.method || (tracked && tracked.method) || "GET",
      url: details.url,
      status: null,
      timeMs: Math.max(0, Math.round(endTime - startTime)),
      error: details.error || "Request failed"
    });
  },
  { urls: ["<all_urls>"] }
);

// ── Markdown / Reader Helpers (for URL / llms.txt reads) ──────────────────

function extractOutlineFromMarkdown(markdown) {
  const lines = String(markdown || "").split(/\r?\n/);
  return lines.filter((line) => /^#{1,6}\s+/.test(line.trim())).join("\n");
}

function filterMarkdownSections(markdown, filterText) {
  if (!filterText) return markdown;
  let matcher;
  try {
    const regex = new RegExp(filterText, "i");
    matcher = (text) => regex.test(text);
  } catch (_) {
    const needle = filterText.toLowerCase();
    matcher = (text) => text.toLowerCase().includes(needle);
  }

  const lines = String(markdown || "").split(/\r?\n/);
  const sections = [];
  let current = { heading: "", lines: [] };
  for (const line of lines) {
    if (/^#{1,6}\s+/.test(line)) {
      if (current.heading || current.lines.length > 0) {
        sections.push(current);
      }
      current = { heading: line, lines: [line] };
    } else {
      current.lines.push(line);
    }
  }
  if (current.heading || current.lines.length > 0) {
    sections.push(current);
  }

  const kept = sections.filter((sec) => matcher(sec.lines.join("\n")));
  return kept.map((sec) => sec.lines.join("\n")).join("\n\n").trim();
}

function htmlStringToMarkdown(html, baseUrl) {
  const parser = new DOMParser();
  const doc = parser.parseFromString(html, "text/html");
  const title = (doc.title || "").trim();
  const root = doc.querySelector("main, article, [role='main']") || doc.body || doc.documentElement;

  const skipTags = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "SVG", "TEMPLATE", "IFRAME", "HEAD"]);

  function walk(node) {
    if (!node) return "";
    if (node.nodeType === Node.TEXT_NODE) {
      return node.nodeValue.replace(/\s+/g, " ");
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return "";
    const tag = node.tagName.toUpperCase();
    if (skipTags.has(tag) || node.hidden || node.getAttribute("aria-hidden") === "true") {
      return "";
    }

    const inner = () => Array.from(node.childNodes).map(walk).join("");

    if (/^H[1-6]$/.test(tag)) {
      const level = Number(tag[1]);
      const text = inner().trim();
      return text ? `\n\n${"#".repeat(level)} ${text}\n\n` : "";
    }
    if (tag === "P" || tag === "DIV" || tag === "SECTION" || tag === "ARTICLE") {
      const text = inner().trim();
      return text ? `\n\n${text}\n\n` : "";
    }
    if (tag === "BR") return "\n";
    if (tag === "HR") return "\n\n---\n\n";
    if (tag === "A") {
      const text = inner().trim();
      let href = node.getAttribute("href") || "";
      if (href && baseUrl) {
        try {
          href = new URL(href, baseUrl).href;
        } catch (_) {}
      }
      if (!text) return "";
      return href ? `[${text}](${href})` : text;
    }
    if (tag === "LI") {
      const text = inner().trim();
      return text ? `\n- ${text}` : "";
    }
    if (tag === "UL" || tag === "OL") {
      const text = inner().trim();
      return text ? `\n\n${text}\n\n` : "";
    }
    if (tag === "PRE") {
      const code = (node.textContent || "").trim();
      return code ? `\n\n\`\`\`\n${code}\n\`\`\`\n\n` : "";
    }
    if (tag === "CODE") {
      const code = (node.textContent || "").trim();
      return code ? `\`${code}\`` : "";
    }
    if (tag === "BLOCKQUOTE") {
      const text = inner().trim();
      return text ? `\n\n> ${text.replace(/\n/g, "\n> ")}\n\n` : "";
    }
    if (tag === "STRONG" || tag === "B") {
      const text = inner().trim();
      return text ? `**${text}**` : "";
    }
    if (tag === "EM" || tag === "I") {
      const text = inner().trim();
      return text ? `*${text}*` : "";
    }
    if (tag === "TR") {
      const cells = Array.from(node.children).map((c) => walk(c).trim().replace(/\|/g, "\\|"));
      return cells.length ? `\n| ${cells.join(" | ")} |` : "";
    }
    if (tag === "TABLE") {
      const text = inner().trim();
      return text ? `\n\n${text}\n\n` : "";
    }
    return inner();
  }

  const md = walk(root)
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  return { title, markdown: md };
}

// ── Image Cropping Helper for selector screenshots ────────────────────────

async function cropDataUrl(dataUrl, rect, dpr, format) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const scale = dpr || 1;
      const sx = Math.max(0, Math.round(rect.x * scale));
      const sy = Math.max(0, Math.round(rect.y * scale));
      const sw = Math.max(1, Math.min(img.width - sx, Math.round(rect.width * scale)));
      const sh = Math.max(1, Math.min(img.height - sy, Math.round(rect.height * scale)));
      const canvas = document.createElement("canvas");
      canvas.width = sw;
      canvas.height = sh;
      const ctx = canvas.getContext("2d");
      ctx.drawImage(img, sx, sy, sw, sh, 0, 0, sw, sh);
      const mime = format === "png" ? "image/png" : "image/jpeg";
      resolve(canvas.toDataURL(mime, 0.85));
    };
    img.onerror = () => reject(new Error("Failed to crop screenshot"));
    img.src = dataUrl;
  });
}

// ── Command Dispatcher ────────────────────────────────────────────────────

async function dispatchCommand(msg) {
  const id = msg.id;
  try {
    const result = await executeCommand(msg.cmd || {}, msg.tabId ?? null);
    sendFrame({ type: "res", id, ok: true, result });
  } catch (err) {
    const message = err && err.message ? err.message : String(err);
    sendFrame({ type: "res", id, ok: false, error: message });
  }
}

async function executeCommand(cmd, msgTabId) {
  const action = String(cmd.action || "");
  const effectiveTabId = msgTabId != null ? msgTabId : (cmd.tabId ?? null);

  switch (action) {
    // ── Tabs ────────────────────────────────────────────────────────────
    case "tab_list": {
      const tabs = await browser.tabs.query({});
      return { ok: true, tabs: tabs.map(serializeTab) };
    }

    case "tab_new": {
      const createProps = { active: Boolean(cmd.active) };
      if (cmd.url) {
        createProps.url = normalizeNavUrl(cmd.url);
      }
      const tab = await browser.tabs.create(createProps);
      return {
        ok: true,
        id: tab.id,
        tabId: tab.id,
        active: Boolean(tab.active),
        title: tab.title || "",
        url: tab.url || createProps.url || "",
        status: tab.status || "loading"
      };
    }

    case "tab_switch": {
      if (effectiveTabId == null) {
        throw new Error("tab_switch requires tabId");
      }
      const tab = await browser.tabs.update(Number(effectiveTabId), { active: true });
      try {
        await browser.windows.update(tab.windowId, { focused: true });
      } catch (_) {}
      return {
        ok: true,
        id: tab.id,
        tabId: tab.id,
        active: true,
        title: tab.title || "",
        url: tab.url || ""
      };
    }

    case "tab_close": {
      const tab = await resolveTargetTab(effectiveTabId);
      await browser.tabs.remove(tab.id);
      if (boundTabId === tab.id) boundTabId = null;
      return { ok: true, closed: tab.id };
    }

    case "tab_bind": {
      if (effectiveTabId == null) {
        boundTabId = null;
        return { ok: true, boundTabId: null };
      }
      const tab = await resolveTargetTab(effectiveTabId);
      boundTabId = tab.id;
      return {
        ok: true,
        id: tab.id,
        tabId: tab.id,
        boundTabId: tab.id,
        active: Boolean(tab.active),
        title: tab.title || "",
        url: tab.url || ""
      };
    }

    case "url": {
      const tab = await resolveTargetTab(effectiveTabId);
      return {
        ok: true,
        tabId: tab.id,
        url: tab.url || "",
        title: tab.title || ""
      };
    }

    // ── Navigation ──────────────────────────────────────────────────────
    case "navigate": {
      const tab = await resolveTargetTab(effectiveTabId);
      const targetUrl = normalizeNavUrl(cmd.url);
      const wait = waitForTabComplete(tab.id, 30000, false);
      await browser.tabs.update(tab.id, { url: targetUrl });
      const settled = await wait;
      return {
        ok: true,
        tabId: tab.id,
        url: settled.url || targetUrl,
        title: settled.title || "",
        status: settled.status || "complete"
      };
    }

    case "back": {
      const tab = await resolveTargetTab(effectiveTabId);
      const wait = waitForTabComplete(tab.id, 15000, false);
      await browser.tabs.goBack(tab.id);
      const settled = await wait;
      return {
        ok: true,
        tabId: tab.id,
        url: settled.url || "",
        title: settled.title || "",
        status: settled.status || "complete"
      };
    }

    case "forward": {
      const tab = await resolveTargetTab(effectiveTabId);
      const wait = waitForTabComplete(tab.id, 15000, false);
      await browser.tabs.goForward(tab.id);
      const settled = await wait;
      return {
        ok: true,
        tabId: tab.id,
        url: settled.url || "",
        title: settled.title || "",
        status: settled.status || "complete"
      };
    }

    case "reload": {
      const tab = await resolveTargetTab(effectiveTabId);
      const wait = waitForTabComplete(tab.id, 30000, true);
      await browser.tabs.reload(tab.id);
      const settled = await wait;
      return {
        ok: true,
        tabId: tab.id,
        url: settled.url || "",
        title: settled.title || "",
        status: settled.status || "complete"
      };
    }

    // ── Network / Routes ────────────────────────────────────────────────
    case "requests": {
      let list = capturedRequests.slice();
      if (effectiveTabId != null) {
        list = list.filter((r) => r.tabId === Number(effectiveTabId));
      }
      if (cmd.filter) {
        const matcher = compileUrlPattern(cmd.filter);
        list = list.filter((r) => matcher(r.url));
      }
      const totalCount = list.length;
      if (cmd.limit != null && Number(cmd.limit) > 0) {
        list = list.slice(-Number(cmd.limit));
      }
      if (cmd.clear) {
        if (effectiveTabId != null) {
          for (let i = capturedRequests.length - 1; i >= 0; i--) {
            if (capturedRequests[i].tabId === Number(effectiveTabId)) {
              capturedRequests.splice(i, 1);
            }
          }
        } else {
          capturedRequests.length = 0;
        }
      }
      return {
        ok: true,
        count: totalCount,
        requests: list.map((r) => ({
          method: r.method,
          status: r.status,
          timeMs: r.timeMs,
          url: r.url,
          error: r.error
        }))
      };
    }

    case "route": {
      const urlPattern = String(cmd.urlPattern || "");
      if (!urlPattern) throw new Error("route requires urlPattern");
      const entry = {
        urlPattern,
        matcher: compileUrlPattern(urlPattern),
        status: cmd.status ?? null,
        body: cmd.body,
        contentType: cmd.contentType || null,
        headers: cmd.headers || null,
        abort: Boolean(cmd.abort),
        redirectUrl: cmd.redirectUrl || null,
        tabId: effectiveTabId != null ? Number(effectiveTabId) : null
      };
      // Replace existing route with identical pattern + tabId
      const existingIdx = activeRoutes.findIndex(
        (r) => r.urlPattern === urlPattern && r.tabId === entry.tabId
      );
      if (existingIdx >= 0) {
        activeRoutes[existingIdx] = entry;
      } else {
        activeRoutes.push(entry);
      }
      return {
        ok: true,
        route: { urlPattern },
        routes: activeRoutes.map((r) => ({ urlPattern: r.urlPattern }))
      };
    }

    case "unroute": {
      const pattern = cmd.urlPattern ? String(cmd.urlPattern) : null;
      let removed = 0;
      for (let i = activeRoutes.length - 1; i >= 0; i--) {
        if (!pattern || activeRoutes[i].urlPattern === pattern) {
          activeRoutes.splice(i, 1);
          removed++;
        }
      }
      return {
        ok: true,
        removed,
        routes: activeRoutes.map((r) => ({ urlPattern: r.urlPattern }))
      };
    }

    // ── Reader ──────────────────────────────────────────────────────────
    case "read": {
      if (cmd.llms) {
        const baseUrl = cmd.url || (await resolveTargetTab(effectiveTabId)).url;
        if (!baseUrl) throw new Error("Cannot determine origin for llms.txt");
        const origin = new URL(baseUrl).origin;
        const llmsUrl = `${origin}/${cmd.llms === "full" ? "llms-full.txt" : "llms.txt"}`;
        const resp = await fetch(llmsUrl);
        if (!resp.ok) {
          throw new Error(`Failed to fetch ${llmsUrl} (HTTP ${resp.status})`);
        }
        let markdown = await resp.text();
        if (cmd.filter) markdown = filterMarkdownSections(markdown, cmd.filter);
        const outline = cmd.outline ? extractOutlineFromMarkdown(markdown) : undefined;
        return {
          ok: true,
          source: "llms",
          url: llmsUrl,
          title: llmsUrl,
          markdown,
          outline,
          truncated: false
        };
      }

      if (cmd.url) {
        const targetUrl = normalizeNavUrl(cmd.url);
        const resp = await fetch(targetUrl);
        if (!resp.ok) {
          throw new Error(`Failed to fetch ${targetUrl} (HTTP ${resp.status})`);
        }
        const contentType = (resp.headers.get("content-type") || "").toLowerCase();
        const rawText = await resp.text();
        let title = targetUrl;
        let markdown = rawText;
        if (contentType.includes("html") || /^\s*<!doctype|\s*<html/i.test(rawText)) {
          const parsed = htmlStringToMarkdown(rawText, targetUrl);
          title = parsed.title || targetUrl;
          markdown = parsed.markdown;
        }
        if (cmd.filter) markdown = filterMarkdownSections(markdown, cmd.filter);
        const outline = cmd.outline ? extractOutlineFromMarkdown(markdown) : undefined;
        return {
          ok: true,
          source: "fetch",
          url: targetUrl,
          title,
          markdown,
          outline,
          truncated: false
        };
      }

      const tab = await resolveTargetTab(effectiveTabId);
      return await sendToPage(tab, cmd);
    }

    // ── Screenshot ──────────────────────────────────────────────────────
    case "screenshot": {
      const tab = await resolveTargetTab(effectiveTabId);
      const format = cmd.format === "png" ? "png" : "jpeg";
      const annotate = cmd.annotate !== false;
      const notes = [];

      let previousActiveTabId = null;
      if (!tab.active) {
        const current = await browser.tabs.query({ active: true, windowId: tab.windowId });
        if (current.length > 0) previousActiveTabId = current[0].id;
        await browser.tabs.update(tab.id, { active: true });
      }

      let annotations = [];
      let clipRect = null;
      let dpr = 1;
      const canScript = isScriptableUrl(tab.url);

      try {
        if (canScript && (annotate || cmd.selector)) {
          const prep = await sendToPage(tab, {
            action: "annotate_show",
            annotate,
            selector: cmd.selector || null
          });
          annotations = prep.annotations || [];
          clipRect = prep.clipRect || null;
          dpr = prep.dpr || 1;
        } else if (annotate && !canScript) {
          notes.push("Annotations skipped on restricted browser page.");
        }

        if (cmd.fullPage) {
          notes.push("Captured visible viewport (full-page stitching is not enabled on this host).");
        }

        const captureOpts = format === "png"
          ? { format: "png" }
          : { format: "jpeg", quality: 85 };
        let dataUrl = await browser.tabs.captureVisibleTab(tab.windowId, captureOpts);

        if (clipRect && clipRect.width > 0 && clipRect.height > 0) {
          dataUrl = await cropDataUrl(dataUrl, clipRect, dpr, format);
        }

        const comma = dataUrl.indexOf(",");
        const base64 = comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;

        return {
          ok: true,
          tabId: tab.id,
          base64,
          format,
          annotate,
          annotations,
          notes
        };
      } finally {
        if (canScript && annotate) {
          try {
            await sendToPage(tab, { action: "annotate_hide" });
          } catch (_) {}
        }
        if (previousActiveTabId != null) {
          try {
            await browser.tabs.update(previousActiveTabId, { active: true });
          } catch (_) {}
        }
      }
    }

    // ── Press (with Enter navigation wait) ──────────────────────────────
    case "press": {
      const tab = await resolveTargetTab(effectiveTabId);
      if (cmd.key === "Enter") {
        let navStarted = false;
        const onUpdated = (updatedId, changeInfo) => {
          if (updatedId === tab.id && changeInfo.status === "loading") {
            navStarted = true;
          }
        };
        const onBeforeNav = (details) => {
          if (details.tabId === tab.id && details.frameId === 0) {
            navStarted = true;
          }
        };
        browser.tabs.onUpdated.addListener(onUpdated);
        if (browser.webNavigation && browser.webNavigation.onBeforeNavigate) {
          browser.webNavigation.onBeforeNavigate.addListener(onBeforeNav);
        }
        try {
          const result = await sendToPage(tab, cmd);
          await new Promise((r) => setTimeout(r, 300));
          if (navStarted) {
            const settled = await waitForTabComplete(tab.id, 15000, false);
            result.navigated = true;
            result.navigationStatus = settled.status || "complete";
          }
          return result;
        } finally {
          browser.tabs.onUpdated.removeListener(onUpdated);
          if (browser.webNavigation && browser.webNavigation.onBeforeNavigate) {
            browser.webNavigation.onBeforeNavigate.removeListener(onBeforeNav);
          }
        }
      }
      return await sendToPage(tab, cmd);
    }

    // ── Page DOM / Snapshot / Interaction / Wait / Eval ─────────────────
    case "snapshot":
    case "diff_snapshot":
    case "click":
    case "dblclick":
    case "contextmenu":
    case "fill":
    case "type":
    case "select":
    case "check":
    case "uncheck":
    case "hover":
    case "scroll":
    case "focus":
    case "locate":
    case "wait":
    case "evaluate": {
      const tab = await resolveTargetTab(effectiveTabId);
      return await sendToPage(tab, cmd);
    }

    default:
      throw new Error(`Unsupported command action: ${action || "(empty)"}`);
  }
}

function normalizeNavUrl(raw) {
  const text = String(raw || "").trim();
  if (!text) throw new Error("URL must be non-empty");
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(text)) {
    return text;
  }
  return `https://${text}`;
}

// ── Startup ───────────────────────────────────────────────────────────────

async function init() {
  try {
    const stored = await browser.storage.local.get([STORAGE_KEY_SECRET, STORAGE_KEY_URL]);
    if (typeof stored[STORAGE_KEY_SECRET] === "string") {
      config.secret = stored[STORAGE_KEY_SECRET].trim();
    }
    if (typeof stored[STORAGE_KEY_URL] === "string" && stored[STORAGE_KEY_URL].trim()) {
      try {
        config.url = normalizeHubUrl(stored[STORAGE_KEY_URL]);
      } catch (_) {
        config.url = DEFAULT_HUB_URL;
      }
    }
  } catch (err) {
    console.error("[Gecko Orange] Failed to read storage:", err);
  }
  updateBadge();
  connect();
}

init();
