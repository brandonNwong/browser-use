"use strict";

(() => {
  if (window.__geckoOrangeLoaded) return;
  window.__geckoOrangeLoaded = true;

  const state = {
    elToRef: new WeakMap(),
    refToEl: new Map(),
    nextRef: 1,
    revision: 0,
    lastSnapshot: null,
    lastSnapshotOpts: { interactive: true, compact: true, urls: false, selector: null, maxChars: 11488 }
  };

  // ── Element Refs & Resolution ─────────────────────────────────────────

  function getOrAssignRef(el) {
    let ref = state.elToRef.get(el);
    if (!ref) {
      ref = `e${state.nextRef++}`;
      state.elToRef.set(el, ref);
      state.refToEl.set(ref, el);
    }
    return ref;
  }

  function isRefString(selector) {
    return typeof selector === "string" && /^@?e\d+$/.test(selector.trim());
  }

  function queryDeep(selector, root = document) {
    try {
      const direct = root.querySelector(selector);
      if (direct) return direct;
    } catch (_) {
      return null;
    }
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    let node = walker.nextNode();
    while (node) {
      if (node.shadowRoot) {
        const hit = queryDeep(selector, node.shadowRoot);
        if (hit) return hit;
      }
      if (node.tagName === "IFRAME") {
        try {
          if (node.contentDocument) {
            const hit = queryDeep(selector, node.contentDocument);
            if (hit) return hit;
          }
        } catch (_) {}
      }
      node = walker.nextNode();
    }
    return null;
  }

  function resolveElement(selector) {
    if (!selector || typeof selector !== "string") {
      throw new Error("Selector is required");
    }
    const trimmed = selector.trim();

    if (isRefString(trimmed)) {
      const key = trimmed.startsWith("@") ? trimmed.slice(1) : trimmed;
      const el = state.refToEl.get(key);
      if (!el || !el.isConnected) {
        throw new Error(`Element @${key} is no longer attached to the page. Call browser_snapshot again.`);
      }
      return el;
    }

    if (trimmed.startsWith("xpath=")) {
      const expr = trimmed.slice(6);
      const result = document.evaluate(
        expr,
        document,
        null,
        XPathResult.FIRST_ORDERED_NODE_TYPE,
        null
      );
      const node = result.singleNodeValue;
      if (!node || node.nodeType !== Node.ELEMENT_NODE) {
        throw new Error(`No element matched XPath: ${expr}`);
      }
      return node;
    }

    const el = queryDeep(trimmed, document);
    if (!el) {
      throw new Error(`No element matched selector: ${trimmed}`);
    }
    return el;
  }

  // ── Visibility, Roles & Accessible Names ──────────────────────────────

  function isElementVisible(el) {
    if (!el || !el.isConnected) return false;
    if (el.hidden || el.getAttribute("aria-hidden") === "true") return false;
    const style = window.getComputedStyle(el);
    if (!style || style.display === "none" || style.visibility === "hidden" || parseFloat(style.opacity) === 0) {
      return false;
    }
    const rect = el.getBoundingClientRect();
    return rect.width > 0 || rect.height > 0;
  }

  function getRole(el) {
    const explicit = el.getAttribute("role");
    if (explicit) return explicit.trim().split(/\s+/)[0];

    const tag = el.tagName.toLowerCase();
    switch (tag) {
      case "a": return el.hasAttribute("href") ? "link" : "generic";
      case "button": return "button";
      case "summary": return "button";
      case "textarea": return "textbox";
      case "select": return el.multiple ? "listbox" : "combobox";
      case "option": return "option";
      case "img": return "img";
      case "nav": return "navigation";
      case "main": return "main";
      case "header": return "banner";
      case "footer": return "contentinfo";
      case "aside": return "complementary";
      case "form": return "form";
      case "table": return "table";
      case "tr": return "row";
      case "th": return "columnheader";
      case "td": return "cell";
      case "ul":
      case "ol": return "list";
      case "li": return "listitem";
      case "dialog": return "dialog";
      case "h1":
      case "h2":
      case "h3":
      case "h4":
      case "h5":
      case "h6": return "heading";
      case "input": {
        const type = (el.getAttribute("type") || "text").toLowerCase();
        if (type === "checkbox") return "checkbox";
        if (type === "radio") return "radio";
        if (type === "button" || type === "submit" || type === "reset") return "button";
        if (type === "search") return "searchbox";
        if (type === "number") return "spinbutton";
        if (type === "range") return "slider";
        if (type === "hidden") return "none";
        return "textbox";
      }
      default:
        if (el.isContentEditable) return "textbox";
        return "generic";
    }
  }

  const INTERACTIVE_ROLES = new Set([
    "button", "link", "textbox", "searchbox", "checkbox", "radio",
    "combobox", "listbox", "option", "menuitem", "menuitemcheckbox",
    "menuitemradio", "tab", "switch", "slider", "spinbutton", "treeitem"
  ]);

  function isInteractive(el, role) {
    if (INTERACTIVE_ROLES.has(role)) return true;
    const tag = el.tagName.toLowerCase();
    if (tag === "button" || tag === "select" || tag === "textarea" || tag === "summary") return true;
    if (tag === "input" && (el.getAttribute("type") || "").toLowerCase() !== "hidden") return true;
    if (tag === "a" && el.hasAttribute("href")) return true;
    if (el.isContentEditable) return true;
    const tabIndex = el.getAttribute("tabindex");
    if (tabIndex !== null && tabIndex !== "-1") return true;
    if (el.hasAttribute("onclick")) return true;
    return false;
  }

  function cleanText(text, maxLen = 180) {
    const collapsed = String(text || "").replace(/\s+/g, " ").trim();
    if (collapsed.length <= maxLen) return collapsed;
    return collapsed.slice(0, maxLen) + "…";
  }

  function getAccessibleName(el) {
    const labelledBy = el.getAttribute("aria-labelledby");
    if (labelledBy) {
      const parts = labelledBy
        .trim()
        .split(/\s+/)
        .map((id) => {
          const ref = document.getElementById(id);
          return ref ? ref.textContent : "";
        })
        .filter(Boolean);
      if (parts.length) return cleanText(parts.join(" "));
    }

    const ariaLabel = el.getAttribute("aria-label");
    if (ariaLabel && ariaLabel.trim()) return cleanText(ariaLabel);

    const tag = el.tagName.toLowerCase();
    if (tag === "input" || tag === "textarea" || tag === "select") {
      if (el.labels && el.labels.length > 0) {
        const labelText = Array.from(el.labels).map((l) => l.textContent).join(" ");
        if (labelText.trim()) return cleanText(labelText);
      }
      if (tag === "input") {
        const type = (el.getAttribute("type") || "text").toLowerCase();
        if ((type === "button" || type === "submit" || type === "reset") && el.value) {
          return cleanText(el.value);
        }
      }
      const placeholder = el.getAttribute("placeholder");
      if (placeholder && placeholder.trim()) return cleanText(placeholder);
    }

    if (tag === "img") {
      const alt = el.getAttribute("alt");
      if (alt && alt.trim()) return cleanText(alt);
    }

    const title = el.getAttribute("title");
    if (title && title.trim() && (tag === "input" || tag === "button" || tag === "a")) {
      const text = cleanText(el.innerText || el.textContent || "");
      return text || cleanText(title);
    }

    const text = cleanText(el.innerText || el.textContent || "");
    if (text) return text;
    if (title && title.trim()) return cleanText(title);
    return "";
  }

  function describeElInfo(el) {
    const ref = getOrAssignRef(el);
    const role = getRole(el);
    const name = getAccessibleName(el);
    return { ref, role, name };
  }

  // ── Accessibility Snapshot ────────────────────────────────────────────

  const SKIP_TAGS = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "SVG", "HEAD", "META", "LINK"]);
  const LANDMARK_COLLAPSE_ROLES = new Set(["navigation", "banner", "contentinfo"]);
  const STRUCTURAL_ROLES = new Set([
    "main", "navigation", "banner", "contentinfo", "complementary",
    "form", "dialog", "heading", "table", "row", "columnheader", "cell", "list", "listitem", "img"
  ]);

  function buildSnapshotTree(rootEl, opts) {
    const interactiveOnly = opts.interactive !== false;
    const compact = opts.compact !== false;
    const includeUrls = Boolean(opts.urls);
    const maxChars = opts.maxChars || 11488;

    const lines = [];
    const refs = new Map();
    const landmarkRanges = [];

    function visit(node, depth) {
      if (!node) return;

      if (node.nodeType === Node.TEXT_NODE) {
        if (!interactiveOnly) {
          const text = cleanText(node.nodeValue, 300);
          if (text) {
            lines.push(`${"  ".repeat(depth)}- StaticText ${JSON.stringify(text)}`);
          }
        }
        return;
      }

      if (node.nodeType !== Node.ELEMENT_NODE) return;
      if (node.id === "__geckoOrangeOverlay") return;
      if (SKIP_TAGS.has(node.tagName)) return;
      if (!isElementVisible(node)) return;

      const role = getRole(node);
      if (role === "none" || role === "presentation") return;

      const interactive = isInteractive(node, role);
      const isHeading = role === "heading";
      const isLandmark = LANDMARK_COLLAPSE_ROLES.has(role);
      const isStructural = STRUCTURAL_ROLES.has(role);

      const shouldEmit = interactive ||
        (!interactiveOnly && (isStructural || !compact));

      let nextDepth = depth;
      let landmarkStart = -1;

      if (shouldEmit) {
        if (isLandmark) {
          landmarkStart = lines.length;
        }
        const name = interactive || isHeading || role === "img" || role === "link"
          ? getAccessibleName(node)
          : cleanText(node.getAttribute("aria-label") || "", 120);

        const attrs = [];
        if (interactive) {
          const ref = getOrAssignRef(node);
          attrs.push(`[ref=${ref}]`);
          refs.set(ref, { role, name });
        }
        if (isHeading && /^H[1-6]$/.test(node.tagName)) {
          attrs.push(`[level=${node.tagName[1]}]`);
        }
        if (node.disabled || node.getAttribute("aria-disabled") === "true") {
          attrs.push("[disabled]");
        }
        if (role === "checkbox" || role === "radio" || role === "switch") {
          const checked = Boolean(node.checked || node.getAttribute("aria-checked") === "true");
          if (checked) attrs.push("[checked]");
        }
        const expanded = node.getAttribute("aria-expanded");
        if (expanded === "true" || expanded === "false") {
          attrs.push(`[expanded=${expanded}]`);
        }
        if ((role === "textbox" || role === "searchbox" || role === "combobox" || role === "spinbutton") && "value" in node) {
          const val = String(node.value || "");
          if (val && val !== name) {
            attrs.push(`[value=${JSON.stringify(cleanText(val, 80))}]`);
          }
        }
        if (includeUrls && role === "link" && node.href) {
          attrs.push(`[url=${JSON.stringify(node.href)}]`);
        }

        const namePart = name ? ` ${JSON.stringify(name)}` : "";
        const attrPart = attrs.length ? ` ${attrs.join(" ")}` : "";
        lines.push(`${"  ".repeat(depth)}- ${role}${namePart}${attrPart}`);
        nextDepth = depth + 1;

        // Avoid duplicating inner text for leaf interactive elements or headings
        if (interactive && !node.shadowRoot && node.tagName !== "SELECT" && node.tagName !== "FORM") {
          return;
        }
        if (isHeading) {
          return;
        }
      }

      if (node.shadowRoot) {
        for (const child of node.shadowRoot.childNodes) {
          visit(child, nextDepth);
        }
      }
      if (node.tagName === "IFRAME") {
        try {
          if (node.contentDocument && node.contentDocument.body) {
            visit(node.contentDocument.body, nextDepth);
          }
        } catch (_) {}
        return;
      }

      for (const child of node.childNodes) {
        visit(child, nextDepth);
      }

      if (landmarkStart >= 0 && lines.length > landmarkStart + 1) {
        landmarkRanges.push({ start: landmarkStart, end: lines.length });
      }
    }

    visit(rootEl, 0);

    let landmarksCollapsed = 0;
    let tree = lines.join("\n");

    if (tree.length > maxChars && landmarkRanges.length > 0 && !opts.selector) {
      const removed = new Set();
      for (const range of landmarkRanges) {
        for (let i = range.start + 1; i < range.end; i++) {
          removed.add(i);
          landmarksCollapsed++;
        }
      }
      const prunedLines = lines.filter((_, idx) => !removed.has(idx));
      tree = prunedLines.join("\n");
    }

    return { tree, lines: tree ? tree.split("\n") : [], refs, landmarksCollapsed };
  }

  function computeSnapshotDelta(prev, next) {
    const changes = [];
    for (const [ref, after] of next.refs.entries()) {
      const before = prev.refs.get(ref);
      if (!before) {
        changes.push({ type: "add", ref, role: after.role, name: after.name, after });
      } else if (before.role !== after.role || before.name !== after.name) {
        changes.push({ type: "replace", ref, before, after });
      }
    }
    for (const [ref, before] of prev.refs.entries()) {
      if (!next.refs.has(ref)) {
        changes.push({ type: "remove", ref, role: before.role, name: before.name });
      }
    }

    // Minimal line splice diff
    const a = prev.lines;
    const b = next.lines;
    let start = 0;
    while (start < a.length && start < b.length && a[start] === b[start]) start++;
    let endA = a.length;
    let endB = b.length;
    while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
      endA--;
      endB--;
    }

    const treeChange = (start === endA && start === endB)
      ? null
      : {
          startLine: start,
          deleteCount: endA - start,
          lines: b.slice(start, endB)
        };

    return { changes, treeChange };
  }

  function handleSnapshot(cmd, tabId) {
    const rootEl = cmd.selector ? resolveElement(cmd.selector) : (document.body || document.documentElement);
    const opts = {
      interactive: cmd.interactive !== false,
      compact: cmd.compact !== false,
      urls: Boolean(cmd.urls),
      selector: cmd.selector || null,
      maxChars: cmd.maxChars || 11488
    };
    state.lastSnapshotOpts = opts;

    const current = buildSnapshotTree(rootEl, opts);
    const prev = state.lastSnapshot;
    const baseRevision = prev ? prev.revision : 0;

    if (cmd.delta && prev) {
      if (prev.tree === current.tree) {
        return {
          ok: true,
          kind: "unchanged",
          title: document.title || "",
          url: location.href,
          tabId,
          revision: prev.revision,
          baseRevision
        };
      }
      state.revision++;
      const { changes, treeChange } = computeSnapshotDelta(prev, current);
      state.lastSnapshot = { ...current, revision: state.revision };
      return {
        ok: true,
        kind: "delta",
        title: document.title || "",
        url: location.href,
        tabId,
        revision: state.revision,
        baseRevision,
        changes,
        treeChange
      };
    }

    state.revision++;
    state.lastSnapshot = { ...current, revision: state.revision };
    return {
      ok: true,
      kind: "full",
      title: document.title || "",
      url: location.href,
      tabId,
      revision: state.revision,
      tree: current.tree,
      landmarksCollapsed: current.landmarksCollapsed
    };
  }

  function handleDiffSnapshot() {
    const prevLines = state.lastSnapshot ? state.lastSnapshot.lines : [];
    const opts = state.lastSnapshotOpts;
    const rootEl = opts.selector ? resolveElement(opts.selector) : (document.body || document.documentElement);
    const next = buildSnapshotTree(rootEl, opts);
    state.revision++;
    state.lastSnapshot = { ...next, revision: state.revision };

    const prevSet = new Map();
    for (const line of prevLines) prevSet.set(line, (prevSet.get(line) || 0) + 1);
    const nextSet = new Map();
    for (const line of next.lines) nextSet.set(line, (nextSet.get(line) || 0) + 1);

    const diffLines = [];
    let additions = 0;
    let removals = 0;

    for (const line of prevLines) {
      const nCount = nextSet.get(line) || 0;
      if (nCount > 0) {
        nextSet.set(line, nCount - 1);
      } else {
        diffLines.push(`- ${line}`);
        removals++;
      }
    }
    for (const line of next.lines) {
      const pCount = prevSet.get(line) || 0;
      if (pCount > 0) {
        prevSet.set(line, pCount - 1);
      } else {
        diffLines.push(`+ ${line}`);
        additions++;
      }
    }

    return {
      ok: true,
      additions,
      removals,
      diff: diffLines.join("\n")
    };
  }

  // ── Interactions ──────────────────────────────────────────────────────

  function dispatchMouseSequence(el, kind) {
    const rect = el.getBoundingClientRect();
    const clientX = rect.left + rect.width / 2;
    const clientY = rect.top + rect.height / 2;
    const common = { bubbles: true, cancelable: true, view: window, clientX, clientY };

    if (kind === "contextmenu") {
      el.dispatchEvent(new PointerEvent("pointerdown", { ...common, button: 2, buttons: 2 }));
      el.dispatchEvent(new MouseEvent("mousedown", { ...common, button: 2, buttons: 2 }));
      el.dispatchEvent(new PointerEvent("pointerup", { ...common, button: 2, buttons: 0 }));
      el.dispatchEvent(new MouseEvent("mouseup", { ...common, button: 2, buttons: 0 }));
      el.dispatchEvent(new MouseEvent("contextmenu", { ...common, button: 2 }));
      return;
    }

    el.dispatchEvent(new PointerEvent("pointerdown", { ...common, button: 0, buttons: 1 }));
    el.dispatchEvent(new MouseEvent("mousedown", { ...common, button: 0, buttons: 1 }));
    el.dispatchEvent(new PointerEvent("pointerup", { ...common, button: 0, buttons: 0 }));
    el.dispatchEvent(new MouseEvent("mouseup", { ...common, button: 0, buttons: 0 }));
    if (typeof el.click === "function") {
      el.click();
    } else {
      el.dispatchEvent(new MouseEvent("click", { ...common, button: 0 }));
    }

    if (kind === "dblclick") {
      el.dispatchEvent(new MouseEvent("dblclick", { ...common, button: 0, detail: 2 }));
    }
  }

  function handleClick(cmd) {
    const el = resolveElement(cmd.selector);
    if (!cmd.force) {
      el.scrollIntoView({ block: "center", inline: "nearest" });
      if (!isElementVisible(el)) {
        throw new Error(`Element ${cmd.selector} is not visible (pass force=true to bypass)`);
      }
      if (el.disabled) {
        throw new Error(`Element ${cmd.selector} is disabled`);
      }
    }
    if (typeof el.focus === "function") {
      try { el.focus({ preventScroll: true }); } catch (_) {}
    }
    dispatchMouseSequence(el, cmd.action);
    return { ok: true, ...describeElInfo(el) };
  }

  function setNativeValue(el, value) {
    const proto = el instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : el instanceof HTMLInputElement
        ? HTMLInputElement.prototype
        : Object.getPrototypeOf(el);
    const descriptor = proto && Object.getOwnPropertyDescriptor(proto, "value");
    if (descriptor && typeof descriptor.set === "function") {
      descriptor.set.call(el, value);
    } else {
      el.value = value;
    }
  }

  async function handleFillOrType(cmd) {
    const el = resolveElement(cmd.selector);
    const text = String(cmd.value ?? "");
    el.scrollIntoView({ block: "center", inline: "nearest" });
    el.focus();

    const isEditable = el.isContentEditable && !(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement);

    if (cmd.action === "type") {
      const delay = Math.max(0, Number(cmd.delay || 0));
      let current = isEditable ? (el.textContent || "") : String(el.value || "");
      for (const ch of text) {
        el.dispatchEvent(new KeyboardEvent("keydown", { key: ch, bubbles: true, cancelable: true }));
        current += ch;
        if (isEditable) {
          el.textContent = current;
        } else {
          setNativeValue(el, current);
        }
        el.dispatchEvent(new InputEvent("input", { data: ch, inputType: "insertText", bubbles: true }));
        el.dispatchEvent(new KeyboardEvent("keyup", { key: ch, bubbles: true, cancelable: true }));
        if (delay > 0) {
          await new Promise((r) => setTimeout(r, delay));
        }
      }
      el.dispatchEvent(new Event("change", { bubbles: true }));
    } else {
      if (isEditable) {
        el.textContent = text;
      } else {
        setNativeValue(el, text);
      }
      el.dispatchEvent(new InputEvent("input", { data: text, inputType: "insertReplacementText", bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
    }

    return { ok: true, ...describeElInfo(el), value: text };
  }

  function parseKeyChord(chord) {
    const parts = String(chord || "").split("+").map((p) => p.trim()).filter(Boolean);
    const mods = { ctrlKey: false, metaKey: false, shiftKey: false, altKey: false };
    let key = parts[parts.length - 1] || "";
    for (let i = 0; i < parts.length - 1; i++) {
      const lower = parts[i].toLowerCase();
      if (lower === "control" || lower === "ctrl") mods.ctrlKey = true;
      else if (lower === "meta" || lower === "cmd" || lower === "command") mods.metaKey = true;
      else if (lower === "shift") mods.shiftKey = true;
      else if (lower === "alt" || lower === "option") mods.altKey = true;
    }
    return { key, mods };
  }

  function handlePress(cmd) {
    const el = cmd.selector === "body"
      ? (document.activeElement && document.activeElement !== document.body ? document.activeElement : document.body)
      : resolveElement(cmd.selector);
    if (cmd.selector !== "body" && typeof el.focus === "function") {
      el.focus();
    }
    const { key, mods } = parseKeyChord(cmd.key);
    const opts = { key, bubbles: true, cancelable: true, ...mods };
    const downNotCancelled = el.dispatchEvent(new KeyboardEvent("keydown", opts));
    if (key.length === 1 || key === "Enter") {
      el.dispatchEvent(new KeyboardEvent("keypress", opts));
    }
    if (key === "Enter" && downNotCancelled) {
      if (el instanceof HTMLInputElement && el.form) {
        if (typeof el.form.requestSubmit === "function") {
          el.form.requestSubmit();
        } else {
          el.form.submit();
        }
      } else if (el instanceof HTMLButtonElement || (el instanceof HTMLAnchorElement && el.href)) {
        el.click();
      }
    }
    el.dispatchEvent(new KeyboardEvent("keyup", opts));
    const info = cmd.selector === "body" ? { ref: null, role: "", name: "" } : describeElInfo(el);
    return { ok: true, ...info };
  }

  function handleSelect(cmd) {
    const el = resolveElement(cmd.selector);
    if (!(el instanceof HTMLSelectElement)) {
      throw new Error(`Element ${cmd.selector} is not a <select> element`);
    }
    const wanted = Array.isArray(cmd.value) ? cmd.value.map(String) : [String(cmd.value ?? "")];
    let matched = 0;
    for (const option of el.options) {
      const hit = wanted.includes(option.value) || wanted.includes(option.textContent.trim());
      if (el.multiple) {
        option.selected = hit;
      } else if (hit && matched === 0) {
        option.selected = true;
      }
      if (hit) matched++;
    }
    if (matched === 0) {
      throw new Error(`No <option> matched ${JSON.stringify(cmd.value)} in ${cmd.selector}`);
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return { ok: true, ...describeElInfo(el), value: el.value };
  }

  function handleCheck(cmd, desired) {
    const el = resolveElement(cmd.selector);
    el.scrollIntoView({ block: "center", inline: "nearest" });
    if (el instanceof HTMLInputElement && (el.type === "checkbox" || el.type === "radio")) {
      if (el.checked !== desired) {
        el.click();
        if (el.checked !== desired) {
          el.checked = desired;
          el.dispatchEvent(new Event("input", { bubbles: true }));
          el.dispatchEvent(new Event("change", { bubbles: true }));
        }
      }
      return { ok: true, ...describeElInfo(el), checked: el.checked };
    }
    const ariaChecked = el.getAttribute("aria-checked") === "true";
    if (ariaChecked !== desired) {
      el.click();
    }
    return { ok: true, ...describeElInfo(el), checked: el.getAttribute("aria-checked") === "true" };
  }

  function handleHover(cmd) {
    const el = resolveElement(cmd.selector);
    el.scrollIntoView({ block: "center", inline: "nearest" });
    const rect = el.getBoundingClientRect();
    const common = {
      bubbles: true,
      cancelable: true,
      view: window,
      clientX: rect.left + rect.width / 2,
      clientY: rect.top + rect.height / 2
    };
    el.dispatchEvent(new PointerEvent("pointerover", common));
    el.dispatchEvent(new PointerEvent("pointerenter", { ...common, bubbles: false }));
    el.dispatchEvent(new MouseEvent("mouseover", common));
    el.dispatchEvent(new MouseEvent("mouseenter", { ...common, bubbles: false }));
    el.dispatchEvent(new MouseEvent("mousemove", common));
    return { ok: true, ...describeElInfo(el) };
  }

  function handleFocus(cmd) {
    const el = resolveElement(cmd.selector);
    el.scrollIntoView({ block: "center", inline: "nearest" });
    el.focus();
    return { ok: true, ...describeElInfo(el) };
  }

  function handleScroll(cmd) {
    if (cmd.selector) {
      const el = resolveElement(cmd.selector);
      el.scrollIntoView({ block: "center", inline: "nearest" });
      return {
        ok: true,
        ...describeElInfo(el),
        scrollX: Math.round(window.scrollX),
        scrollY: Math.round(window.scrollY)
      };
    }
    const y = Number(cmd.y || 0);
    window.scrollTo(window.scrollX, y);
    return {
      ok: true,
      scrollX: Math.round(window.scrollX),
      scrollY: Math.round(window.scrollY)
    };
  }

  // ── Locate ────────────────────────────────────────────────────────────

  function textMatches(candidate, query, exact) {
    const a = String(candidate || "").replace(/\s+/g, " ").trim();
    const b = String(query || "").replace(/\s+/g, " ").trim();
    if (!a || !b) return false;
    return exact ? a === b : a.toLowerCase().includes(b.toLowerCase());
  }

  function collectAllElements(root = document, out = []) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    let node = walker.nextNode();
    while (node) {
      if (node.id !== "__geckoOrangeOverlay" && !SKIP_TAGS.has(node.tagName)) {
        out.push(node);
        if (node.shadowRoot) collectAllElements(node.shadowRoot, out);
        if (node.tagName === "IFRAME") {
          try {
            if (node.contentDocument) collectAllElements(node.contentDocument, out);
          } catch (_) {}
        }
      }
      node = walker.nextNode();
    }
    return out;
  }

  function handleLocate(cmd) {
    const by = cmd.by;
    const exact = Boolean(cmd.exact);
    const elements = collectAllElements().filter(isElementVisible);
    const hits = [];

    for (const el of elements) {
      if (by === "role") {
        const role = getRole(el);
        const wantedRole = String(cmd.role || "").toLowerCase();
        if (role.toLowerCase() !== wantedRole) continue;
        if (cmd.name && !textMatches(getAccessibleName(el), cmd.name, exact)) continue;
        hits.push(el);
      } else if (by === "text") {
        const text = el.innerText || el.textContent || "";
        if (textMatches(text, cmd.text, exact)) {
          // Prefer deepest matching element
          const hasChildMatch = Array.from(el.children).some(
            (child) => isElementVisible(child) && textMatches(child.innerText || child.textContent || "", cmd.text, exact)
          );
          if (!hasChildMatch) hits.push(el);
        }
      } else if (by === "label") {
        const label = getAccessibleName(el);
        if (textMatches(label, cmd.label, exact)) hits.push(el);
      } else if (by === "placeholder") {
        if (textMatches(el.getAttribute("placeholder"), cmd.placeholder, exact)) hits.push(el);
      } else if (by === "alt") {
        if (textMatches(el.getAttribute("alt"), cmd.alt, exact)) hits.push(el);
      } else if (by === "title") {
        if (textMatches(el.getAttribute("title"), cmd.title, exact)) hits.push(el);
      } else if (by === "testid") {
        const tid = el.getAttribute("data-testid") || el.getAttribute("data-test-id") || el.getAttribute("data-test");
        if (textMatches(tid, cmd.testId, exact)) hits.push(el);
      }
    }

    if (hits.length === 0) {
      throw new Error(`No element located by ${by}`);
    }
    const first = hits[0];
    return {
      ok: true,
      ...describeElInfo(first),
      matches: hits.length
    };
  }

  // ── Wait ──────────────────────────────────────────────────────────────

  async function handleWait(cmd) {
    const started = Date.now();
    const timeoutMs = Math.max(0, Number(cmd.timeoutMs ?? 10000));
    const hasSelector = Boolean(cmd.selector);
    const hasText = Boolean(cmd.text);

    if (!hasSelector && !hasText) {
      if (document.readyState === "complete") {
        await new Promise((r) => setTimeout(r, Math.min(timeoutMs, 250)));
        return { ok: true, waited: Date.now() - started };
      }
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, timeoutMs);
        const onLoad = () => {
          clearTimeout(timer);
          resolve();
        };
        window.addEventListener("load", onLoad, { once: true });
      });
      return { ok: true, waited: Date.now() - started };
    }

    const desiredState = cmd.state || "visible";

    function checkCondition() {
      if (hasSelector) {
        let el = null;
        try {
          el = resolveElement(cmd.selector);
        } catch (_) {
          el = null;
        }
        const attached = Boolean(el && el.isConnected);
        const visible = attached && isElementVisible(el);
        if (desiredState === "attached") return attached;
        if (desiredState === "detached") return !attached;
        if (desiredState === "visible") return visible;
        if (desiredState === "hidden") return !visible;
        return visible;
      }

      const bodyText = document.body ? (document.body.innerText || document.body.textContent || "") : "";
      const found = bodyText.toLowerCase().includes(String(cmd.text).toLowerCase());
      if (desiredState === "hidden" || desiredState === "detached") return !found;
      return found;
    }

    if (checkCondition()) {
      return { ok: true, state: desiredState, waited: Date.now() - started };
    }

    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        observer.disconnect();
        clearInterval(interval);
        reject(new Error(`Timed out after ${timeoutMs}ms waiting for ${cmd.selector || cmd.text} (${desiredState})`));
      }, timeoutMs);

      const done = () => {
        if (!checkCondition()) return;
        clearTimeout(timer);
        clearInterval(interval);
        observer.disconnect();
        resolve();
      };

      const observer = new MutationObserver(done);
      observer.observe(document.documentElement, { childList: true, subtree: true, attributes: true, characterData: true });
      const interval = setInterval(done, 100);
    });

    return { ok: true, state: desiredState, waited: Date.now() - started };
  }

  // ── Reader (Active Tab Markdown Extraction) ───────────────────────────

  function extractMarkdownFromDom(root) {
    function walk(node) {
      if (!node) return "";
      if (node.nodeType === Node.TEXT_NODE) {
        return node.nodeValue.replace(/\s+/g, " ");
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return "";
      if (node.id === "__geckoOrangeOverlay") return "";
      const tag = node.tagName.toUpperCase();
      if (SKIP_TAGS.has(tag) || !isElementVisible(node)) return "";

      if (tag === "IFRAME") {
        try {
          if (node.contentDocument && node.contentDocument.body) {
            return `\n\n${walk(node.contentDocument.body)}\n\n`;
          }
        } catch (_) {}
        return "";
      }

      const walkChildren = () => {
        let out = "";
        if (node.shadowRoot) {
          for (const child of node.shadowRoot.childNodes) out += walk(child);
        }
        for (const child of node.childNodes) out += walk(child);
        return out;
      };

      if (/^H[1-6]$/.test(tag)) {
        const text = walkChildren().trim();
        return text ? `\n\n${"#".repeat(Number(tag[1]))} ${text}\n\n` : "";
      }
      if (tag === "P" || tag === "SECTION" || tag === "ARTICLE" || tag === "DIV") {
        const text = walkChildren().trim();
        return text ? `\n\n${text}\n\n` : "";
      }
      if (tag === "BR") return "\n";
      if (tag === "HR") return "\n\n---\n\n";
      if (tag === "A") {
        const text = walkChildren().trim();
        const href = node.href || "";
        if (!text) return "";
        return href ? `[${text}](${href})` : text;
      }
      if (tag === "LI") {
        const text = walkChildren().trim();
        return text ? `\n- ${text}` : "";
      }
      if (tag === "UL" || tag === "OL") {
        const text = walkChildren().trim();
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
        const text = walkChildren().trim();
        return text ? `\n\n> ${text.replace(/\n/g, "\n> ")}\n\n` : "";
      }
      if (tag === "STRONG" || tag === "B") {
        const text = walkChildren().trim();
        return text ? `**${text}**` : "";
      }
      if (tag === "EM" || tag === "I") {
        const text = walkChildren().trim();
        return text ? `*${text}*` : "";
      }
      if (tag === "TR") {
        const cells = Array.from(node.children).map((c) => walk(c).trim().replace(/\|/g, "\\|"));
        return cells.length ? `\n| ${cells.join(" | ")} |` : "";
      }
      if (tag === "TABLE") {
        const text = walkChildren().trim();
        return text ? `\n\n${text}\n\n` : "";
      }
      return walkChildren();
    }

    return walk(root)
      .replace(/[ \t]+\n/g, "\n")
      .replace(/\n{3,}/g, "\n\n")
      .trim();
  }

  function filterMarkdown(markdown, filterText) {
    if (!filterText) return markdown;
    let matcher;
    try {
      const regex = new RegExp(filterText, "i");
      matcher = (s) => regex.test(s);
    } catch (_) {
      const needle = filterText.toLowerCase();
      matcher = (s) => s.toLowerCase().includes(needle);
    }
    const lines = markdown.split(/\r?\n/);
    const sections = [];
    let current = { lines: [] };
    for (const line of lines) {
      if (/^#{1,6}\s+/.test(line)) {
        if (current.lines.length) sections.push(current);
        current = { lines: [line] };
      } else {
        current.lines.push(line);
      }
    }
    if (current.lines.length) sections.push(current);
    return sections
      .filter((sec) => matcher(sec.lines.join("\n")))
      .map((sec) => sec.lines.join("\n"))
      .join("\n\n")
      .trim();
  }

  function handleRead(cmd) {
    const main = document.querySelector("main, article, [role='main']") || document.body || document.documentElement;
    let markdown = extractMarkdownFromDom(main);
    if (!markdown && main !== document.body && document.body) {
      markdown = extractMarkdownFromDom(document.body);
    }
    if (cmd.filter) {
      markdown = filterMarkdown(markdown, cmd.filter);
    }
    const outline = cmd.outline
      ? markdown.split(/\r?\n/).filter((l) => /^#{1,6}\s+/.test(l.trim())).join("\n")
      : undefined;
    return {
      ok: true,
      source: "tab",
      url: location.href,
      title: document.title || "",
      markdown,
      outline,
      truncated: false
    };
  }

  // ── Set-of-Mark Screenshot Annotations ────────────────────────────────

  function removeAnnotations() {
    const existing = document.getElementById("__geckoOrangeOverlay");
    if (existing) existing.remove();
  }

  function handleAnnotateShow(cmd) {
    removeAnnotations();
    let clipRect = null;
    if (cmd.selector) {
      const target = resolveElement(cmd.selector);
      target.scrollIntoView({ block: "center", inline: "nearest" });
      const r = target.getBoundingClientRect();
      clipRect = { x: r.left, y: r.top, width: r.width, height: r.height };
    }

    const annotations = [];
    if (cmd.annotate) {
      const overlay = document.createElement("div");
      overlay.id = "__geckoOrangeOverlay";
      overlay.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647;";

      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const elements = collectAllElements();
      let count = 0;

      for (const el of elements) {
        if (count >= 150) break;
        if (!isElementVisible(el)) continue;
        const role = getRole(el);
        if (!isInteractive(el, role)) continue;
        const rect = el.getBoundingClientRect();
        if (rect.bottom < 0 || rect.top > vh || rect.right < 0 || rect.left > vw) continue;
        if (rect.width < 4 || rect.height < 4) continue;

        const ref = getOrAssignRef(el);
        const label = ref.replace(/^e/, "");
        const name = getAccessibleName(el);
        count++;

        const box = document.createElement("div");
        box.style.cssText =
          `position:fixed;left:${rect.left}px;top:${rect.top}px;width:${rect.width}px;height:${rect.height}px;` +
          "border:1.5px solid #FF5200;box-sizing:border-box;background:rgba(255,107,26,0.06);";

        const badge = document.createElement("div");
        badge.textContent = label;
        badge.style.cssText =
          `position:fixed;left:${Math.max(0, rect.left)}px;top:${Math.max(0, rect.top - 16)}px;` +
          "background:#FF5200;color:#FFFFFF;font:700 10px/14px ui-monospace,Consolas,monospace;" +
          "padding:0 4px;border-radius:3px;box-shadow:0 1px 2px rgba(0,0,0,0.35);";

        overlay.appendChild(box);
        overlay.appendChild(badge);
        annotations.push({ label, ref, role, name });
      }
      document.documentElement.appendChild(overlay);
    }

    return {
      ok: true,
      annotations,
      clipRect,
      dpr: window.devicePixelRatio || 1
    };
  }

  // ── Page JS Evaluation (MAIN world with isolated CSP fallback) ────────

  function serializeEvalValue(value) {
    if (value === undefined) return undefined;
    if (value === null || typeof value === "boolean" || typeof value === "number" || typeof value === "string") {
      return value;
    }
    try {
      return JSON.parse(JSON.stringify(value));
    } catch (_) {
      return String(value);
    }
  }

  async function evaluateMainWorld(source, isExpression) {
    const eventName = `__geckoOrangeEval_${Date.now()}_${Math.random().toString(36).slice(2)}`;
    const body = isExpression ? `return (\n${source}\n);` : source;

    return new Promise((resolve) => {
      let handled = false;
      let ran = false;

      const onReply = (ev) => {
        handled = true;
        window.removeEventListener(eventName, onReply);
        try {
          resolve(JSON.parse(String(ev.detail || "{}")));
        } catch (err) {
          resolve({ ok: false, error: "Failed to parse evaluation result", world: "main" });
        }
      };
      window.addEventListener(eventName, onReply);

      const script = document.createElement("script");
      script.textContent = `
        (async () => {
          document.documentElement.dataset.geckoOrangeEvalRan = "1";
          const evt = ${JSON.stringify(eventName)};
          const emit = (payload) => {
            window.dispatchEvent(new CustomEvent(evt, { detail: JSON.stringify(payload) }));
          };
          try {
            const value = await (async () => {
              ${body}
            })();
            let serialized;
            if (value === undefined) serialized = undefined;
            else if (value === null || typeof value !== "object") serialized = value;
            else {
              try { serialized = JSON.parse(JSON.stringify(value)); }
              catch (_) { serialized = String(value); }
            }
            emit({ ok: true, result: serialized, world: "main" });
          } catch (err) {
            emit({ ok: false, error: err && err.message ? err.message : String(err), world: "main" });
          }
        })();
      `;
      delete document.documentElement.dataset.geckoOrangeEvalRan;
      (document.head || document.documentElement).appendChild(script);
      script.remove();
      ran = document.documentElement.dataset.geckoOrangeEvalRan === "1";
      delete document.documentElement.dataset.geckoOrangeEvalRan;

      if (!ran) {
        window.removeEventListener(eventName, onReply);
        resolve({ blocked: true });
        return;
      }

      setTimeout(() => {
        if (!handled) {
          window.removeEventListener(eventName, onReply);
          resolve({ ok: false, error: "Evaluation timed out", world: "main" });
        }
      }, 25000);
    });
  }

  async function handleEvaluate(cmd) {
    const isExpression = cmd.expression != null && cmd.code == null;
    const source = isExpression ? String(cmd.expression) : String(cmd.code ?? "");

    const mainAttempt = await evaluateMainWorld(source, isExpression);
    if (!mainAttempt.blocked) {
      if (mainAttempt.ok === false) {
        throw new Error(mainAttempt.error || "Page evaluation failed");
      }
      return { ok: true, result: mainAttempt.result, world: "main" };
    }

    // Fallback when page CSP blocks inline <script>: try wrappedJSObject.eval or isolated eval.
    const body = isExpression ? `return (${source});` : source;
    try {
      if (window.wrappedJSObject && typeof window.wrappedJSObject.eval === "function") {
        const value = await window.wrappedJSObject.eval(`(async () => { ${body} })()`);
        return { ok: true, result: serializeEvalValue(value), world: "main" };
      }
    } catch (_) {
      // Fall through to isolated world error.
    }
    throw new Error("Page Content-Security-Policy blocked script evaluation on this page.");
  }

  // ── Command Router ────────────────────────────────────────────────────

  async function handlePageCommand(cmd, tabId) {
    try {
      switch (cmd.action) {
        case "snapshot": return handleSnapshot(cmd, tabId);
        case "diff_snapshot": return handleDiffSnapshot();
        case "click":
        case "dblclick":
        case "contextmenu": return handleClick(cmd);
        case "fill":
        case "type": return await handleFillOrType(cmd);
        case "press": return handlePress(cmd);
        case "select": return handleSelect(cmd);
        case "check": return handleCheck(cmd, true);
        case "uncheck": return handleCheck(cmd, false);
        case "hover": return handleHover(cmd);
        case "focus": return handleFocus(cmd);
        case "scroll": return handleScroll(cmd);
        case "locate": return handleLocate(cmd);
        case "wait": return await handleWait(cmd);
        case "read": return handleRead(cmd);
        case "annotate_show": return handleAnnotateShow(cmd);
        case "annotate_hide": removeAnnotations(); return { ok: true };
        case "evaluate": return await handleEvaluate(cmd);
        default:
          return { ok: false, error: `Unsupported page action: ${cmd.action}` };
      }
    } catch (err) {
      return { ok: false, error: err && err.message ? err.message : String(err) };
    }
  }

  browser.runtime.onMessage.addListener((msg) => {
    if (!msg || typeof msg !== "object") return undefined;
    if (msg.__geckoOrangePing) {
      return Promise.resolve({ ok: true });
    }
    if (msg.__geckoOrange) {
      return handlePageCommand(msg.cmd || {}, msg.tabId ?? null);
    }
    return undefined;
  });
})();
