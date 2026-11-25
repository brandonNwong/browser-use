"use strict";

const statusPill = document.getElementById("statusPill");
const statusBox = document.getElementById("statusBox");
const secretInput = document.getElementById("secretInput");
const urlInput = document.getElementById("urlInput");
const configForm = document.getElementById("configForm");
const saveBtn = document.getElementById("saveBtn");
const reconnectBtn = document.getElementById("reconnectBtn");
const feedback = document.getElementById("feedback");

function renderStatus(s) {
  if (!s) return;
  urlInput.value = s.url || "ws://127.0.0.1:19455/";
  secretInput.placeholder = s.hasSecret
    ? "Secret saved (paste new secret to replace)"
    : "Paste secret or await OrangeBridge.setSecret(\"...\")";

  statusPill.className = "pill";
  if (s.connected) {
    statusPill.textContent = "LIVE";
    statusPill.classList.add("connected");
    statusBox.innerHTML = `Connected to <strong>${escapeHtml(s.url)}</strong> (protocol ${escapeHtml(s.protocol)}).`;
  } else if (s.state === "unauthorized") {
    statusPill.textContent = "SECRET";
    statusPill.classList.add("error");
    statusBox.innerHTML = `Hub rejected the shared secret at <strong>${escapeHtml(s.url)}</strong>. Paste the current secret from Browser Use.`;
  } else if (!s.hasSecret) {
    statusPill.textContent = "SETUP";
    statusPill.classList.add("connecting");
    statusBox.textContent = "No shared secret configured yet. Paste your Browser Use secret or setup command below.";
  } else {
    statusPill.textContent = "WAITING";
    statusPill.classList.add("connecting");
    const extra = s.lastError ? ` (${escapeHtml(s.lastError)})` : "";
    statusBox.innerHTML = `Waiting for Browser Use hub at <strong>${escapeHtml(s.url)}</strong>${extra}.`;
  }
}

function escapeHtml(text) {
  return String(text || "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function setFeedback(text, kind) {
  feedback.textContent = text || "";
  feedback.className = "feedback" + (kind ? " " + kind : "");
}

async function refresh() {
  try {
    const s = await browser.runtime.sendMessage({ type: "orange_popup_get_status" });
    renderStatus(s);
  } catch (err) {
    setFeedback(String(err), "err");
  }
}

configForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  saveBtn.disabled = true;
  setFeedback("Connecting…", "");
  try {
    const res = await browser.runtime.sendMessage({
      type: "orange_popup_save",
      secret: secretInput.value,
      url: urlInput.value
    });
    if (res && res.status) {
      renderStatus(res.status);
    }
    if (res && res.ok) {
      secretInput.value = "";
      if (res.connected) {
        setFeedback("Connected to Browser Use!", "ok");
      } else if (res.error) {
        setFeedback("Saved, but not connected yet: " + res.error, "err");
      } else {
        setFeedback("Saved. Waiting for Browser Use hub…", "");
      }
    } else {
      setFeedback((res && res.error) || "Failed to save settings.", "err");
    }
  } catch (err) {
    setFeedback(String(err), "err");
  } finally {
    saveBtn.disabled = false;
  }
});

reconnectBtn.addEventListener("click", async () => {
  reconnectBtn.disabled = true;
  setFeedback("Reconnecting…", "");
  try {
    const res = await browser.runtime.sendMessage({ type: "orange_popup_reconnect" });
    if (res && res.status) {
      renderStatus(res.status);
    }
    if (res && res.connected) {
      setFeedback("Connected!", "ok");
    } else {
      setFeedback((res && res.error) || "Hub not reachable yet.", "err");
    }
  } catch (err) {
    setFeedback(String(err), "err");
  } finally {
    reconnectBtn.disabled = false;
  }
});

refresh();
