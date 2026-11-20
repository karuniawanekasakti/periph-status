const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const ICONS = {
  mouse: '<rect x="6" y="3" width="12" height="18" rx="6"/><path d="M12 7v4"/>',
  keyboard:
    '<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/>',
  headset:
    '<path d="M4 15v-3a8 8 0 0 1 16 0v3"/><rect x="3" y="14" width="4" height="6" rx="1.5"/><rect x="17" y="14" width="4" height="6" rx="1.5"/>',
  gamepad:
    '<path d="M7 8h10a5 5 0 0 1 4.6 7l-.6 1.4a2.5 2.5 0 0 1-4.3.4L15 15H9l-1.7 1.8a2.5 2.5 0 0 1-4.3-.4L2.4 15A5 5 0 0 1 7 8z"/><path d="M7 11v3M5.5 12.5h3M16 12h.01M18 11h.01"/>',
  phone: '<rect x="7" y="2" width="10" height="20" rx="2"/><path d="M11 18h2"/>',
  computer: '<rect x="3" y="4" width="18" height="12" rx="1.5"/><path d="M8 20h8M12 16v4"/>',
  other: '<path d="M7 7l10 10-5 5V2l5 5L7 17"/>',
};

const INTERVALS = [15, 30, 60, 300, 900];

let snapshot = null;
let settings = null;
let refreshing = false;

function levelClass(pct) {
  if (pct <= settings.threshold) return "low";
  if (pct <= 50) return "warn";
  return "ok";
}

function renderDevices() {
  const list = $("device-list");
  const empty = $("empty");
  const error = $("error");
  list.replaceChildren();

  if (!snapshot || !snapshot.scanned) {
    empty.textContent = "Scanning for devices…";
    empty.hidden = false;
    return;
  }

  const devices = snapshot.devices.filter((d) => d.connected || settings.showDisconnected);
  for (const d of devices) {
    const li = document.createElement("li");
    li.className = "device" + (d.connected ? "" : " disconnected");

    const kind = document.createElement("div");
    kind.className = "kind";
    kind.innerHTML = `<svg viewBox="0 0 24 24">${ICONS[d.kind] ?? ICONS.other}</svg>`;

    const info = document.createElement("div");
    info.className = "info";
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = d.name;
    name.title = d.name;
    const status = document.createElement("div");
    status.className = "status";
    // For disconnected devices only the last known battery value is available.
    status.textContent = d.connected
      ? "Connected"
      : d.battery == null
        ? "Disconnected"
        : "Disconnected · last known";
    info.append(name, status);

    const level = document.createElement("div");
    level.className = "level";
    const pct = document.createElement("div");
    pct.className = "pct";
    if (d.battery == null) {
      pct.classList.add("none");
      pct.textContent = "—";
      level.title = "This device does not report its battery level";
      level.append(pct);
    } else {
      pct.textContent = `${d.battery}%`;
      level.classList.add(levelClass(d.battery));
      level.title =
        d.source === "gatt"
          ? "Read from the GATT Battery Service"
          : d.source === "dongle"
            ? "Read from the 2.4 GHz receiver"
            : "Reported by Windows";
      const bar = document.createElement("div");
      bar.className = "bar";
      const fill = document.createElement("i");
      fill.style.width = `${d.battery}%`;
      bar.append(fill);
      level.append(pct, bar);
    }

    li.append(kind, info, level);
    list.append(li);
  }

  empty.hidden = devices.length > 0;
  if (devices.length === 0) {
    empty.textContent =
      snapshot.devices.length === 0
        ? "No paired devices found."
        : "No devices connected.";
  }

  error.hidden = !snapshot.error;
  error.textContent = snapshot.error ? `Scan failed: ${snapshot.error}` : "";
}

function renderUpdated() {
  const el = $("updated");
  if (refreshing || !snapshot?.updatedAt) {
    el.textContent = "Scanning…";
    return;
  }
  const secs = Math.max(0, Math.round(Date.now() / 1000 - snapshot.updatedAt));
  el.textContent =
    secs < 10 ? "Updated just now" : secs < 60 ? `Updated ${secs}s ago` : `Updated ${Math.round(secs / 60)} min ago`;
}

function setRefreshing(on) {
  refreshing = on;
  $("btn-refresh").classList.toggle("spinning", on);
  renderUpdated();
}

function applySnapshot(s) {
  snapshot = s;
  setRefreshing(false);
  renderDevices();
}

// ---- settings --------------------------------------------------------------

function fillSettingsForm() {
  $("set-notify").checked = settings.notify;
  $("set-threshold").value = settings.threshold;
  $("threshold-value").textContent = `${settings.threshold}%`;
  const interval = INTERVALS.includes(settings.intervalSecs) ? settings.intervalSecs : 60;
  $("set-interval").value = String(interval);
  $("set-show-disconnected").checked = settings.showDisconnected;
}

function showSettingsError(msg) {
  const el = $("settings-error");
  el.hidden = !msg;
  el.textContent = msg || "";
}

async function saveSettings() {
  const next = {
    threshold: Number($("set-threshold").value),
    intervalSecs: Number($("set-interval").value),
    notify: $("set-notify").checked,
    showDisconnected: $("set-show-disconnected").checked,
  };
  try {
    settings = await invoke("set_settings", { settings: next });
    showSettingsError("");
    renderDevices();
  } catch (e) {
    showSettingsError(String(e));
  }
}

function toggleView(showSettings) {
  $("view-settings").hidden = !showSettings;
  $("view-devices").hidden = showSettings;
  $("btn-settings").classList.toggle("active", showSettings);
  $("title").textContent = showSettings ? "Settings" : "Devices";
}

// ---- init ------------------------------------------------------------------

async function init() {
  settings = await invoke("get_settings");
  fillSettingsForm();
  invoke("get_autostart")
    .then((on) => ($("set-autostart").checked = on))
    .catch(() => ($("set-autostart").disabled = true));

  await listen("devices-updated", (e) => applySnapshot(e.payload));
  applySnapshot(await invoke("get_snapshot"));
  if (!snapshot.scanned) setRefreshing(true);

  $("btn-refresh").addEventListener("click", () => {
    setRefreshing(true);
    invoke("refresh_now");
  });
  $("btn-settings").addEventListener("click", () => toggleView($("view-settings").hidden));

  $("set-threshold").addEventListener("input", (e) => {
    $("threshold-value").textContent = `${e.target.value}%`;
  });
  for (const id of ["set-threshold", "set-interval", "set-notify", "set-show-disconnected"]) {
    $(id).addEventListener("change", saveSettings);
  }
  $("set-autostart").addEventListener("change", async (e) => {
    try {
      await invoke("set_autostart", { enabled: e.target.checked });
      showSettingsError("");
    } catch (err) {
      e.target.checked = !e.target.checked;
      showSettingsError(String(err));
    }
  });
  $("btn-quit").addEventListener("click", () => invoke("quit_app"));

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") invoke("hide_popup");
  });
  // When the popup is reopened, resync the autostart state (it can change from the tray menu).
  window.addEventListener("focus", () => {
    invoke("get_autostart").then((on) => ($("set-autostart").checked = on)).catch(() => {});
    renderUpdated();
  });
  setInterval(renderUpdated, 10_000);
}

init().catch((e) => {
  $("error").hidden = false;
  $("error").textContent = String(e);
});
