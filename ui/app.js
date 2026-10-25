// Chlorum interface. One page, three screens:
//   ?mode=menu     main menu inside the desktop session
//   ?mode=greeter  first run (until it is done), then login
// Every request carries the per-start token given in the URL (?t=...).
"use strict";

const params = new URLSearchParams(location.search);
const TOKEN = params.get("t") || "";
const MODE = params.get("mode") || "menu";

async function api(path, body) {
  const opts = body === undefined
    ? { method: "GET" }
    : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
  const res = await fetch(`/api/${path}?t=${encodeURIComponent(TOKEN)}`, opts);
  let data = {};
  try { data = await res.json(); } catch (e) { /* empty answer */ }
  if (!res.ok && !data.error) data.error = `Request failed (${res.status})`;
  return data;
}

const $ = (id) => document.getElementById(id);
function show(id) {
  for (const s of document.querySelectorAll(".screen")) s.hidden = s.id !== id;
}
function setStatus(el, text, kind) {
  el.textContent = text || "";
  el.className = "status" + (kind ? " " + kind : "");
}

// ---------- Clock: time with seconds, date and time zone ----------
function pad(n) { return String(n).padStart(2, "0"); }
function zoneLabel(d) {
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const off = -d.getTimezoneOffset();
  const sign = off >= 0 ? "+" : "-";
  const a = Math.abs(off);
  return `${zone} (UTC${sign}${pad(Math.floor(a / 60))}:${pad(a % 60)})`;
}
function dateLabel(d) {
  return d.toLocaleDateString("en-US", { weekday: "long", day: "numeric", month: "long", year: "numeric" });
}
function tick() {
  const d = new Date();
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const s = pad(d.getSeconds());
  if ($("time")) $("time").innerHTML = `${hm}<span class="sec">:${s}</span>`;
  if ($("date")) $("date").textContent = dateLabel(d);
  if ($("zone")) $("zone").textContent = zoneLabel(d);
  if ($("login-time")) $("login-time").textContent = hm;
  if ($("login-date")) $("login-date").textContent = `${dateLabel(d)} · ${zoneLabel(d)}`;
}
setInterval(tick, 250);
tick();

// ---------- Power buttons (menu and login) ----------
for (const b of document.querySelectorAll("[data-power]")) {
  b.addEventListener("click", () => power(b.dataset.power));
}
async function power(action) {
  const statusEl = MODE === "menu" ? $("menu-status") : $("login-status");
  const r = await api("power", { action });
  if (r.error) setStatus(statusEl, r.error, "err");
}

// ---------- Icons ----------
const ICONS = {
  browser: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.8 3 2.8 15 0 18M12 3c-2.8 3-2.8 15 0 18"/></svg>',
  terminal: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"><rect x="3" y="4" width="18" height="16"/><path d="M7 9l3 3-3 3M12 15h5"/></svg>',
  update: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"><path d="M20 12a8 8 0 0 1-14.3 4.9M4 12a8 8 0 0 1 14.3-4.9M18.5 3v4.2h-4.2M5.5 21v-4.2h4.2"/></svg>',
};

// ---------- Main menu ----------
const APPS = [
  { id: "firefox", name: "Firefox", icon: "browser" },
  { id: "terminal", name: "Terminal", icon: "terminal" },
  { id: "update", name: "Update Chlorum", icon: "update" },
];

function initMenu(info) {
  show("menu");
  $("menu-user").textContent = info.user ? info.user.toUpperCase() : "";
  const ribbon = $("ribbon");
  ribbon.innerHTML = "";
  APPS.forEach((app, i) => {
    const t = document.createElement("button");
    t.className = "tile";
    t.innerHTML = `<span class="badge">${i + 1}</span>${ICONS[app.icon]}<span class="name">${app.name}</span>`;
    t.addEventListener("mouseenter", () => focusTile(i));
    t.addEventListener("click", () => openApp(i));
    ribbon.appendChild(t);
  });
  const powerBtns = [...document.querySelectorAll("#menu [data-power]")];
  let zone = "ribbon";       // "ribbon" or "power"
  let tileIdx = 0, powerIdx = 0;

  function paint() {
    [...ribbon.children].forEach((t, i) => t.classList.toggle("focus", zone === "ribbon" && i === tileIdx));
    powerBtns.forEach((b, i) => b.classList.toggle("focus", zone === "power" && i === powerIdx));
  }
  function focusTile(i) { zone = "ribbon"; tileIdx = i; paint(); }
  window.focusTile = focusTile;

  async function openApp(i) {
    const app = APPS[i];
    if (app.id === "update") return startUpdate();
    const r = await api("launch", { app: app.id });
    if (r.error) setStatus($("menu-status"), r.error, "err");
  }

  async function startUpdate() {
    setStatus($("menu-status"), "Checking for a new Chlorum version...");
    const r = await api("update", {});
    if (r.error) return setStatus($("menu-status"), r.error, "err");
    pollUpdate();
  }
  async function pollUpdate() {
    const r = await api("update");
    if (r.state === "running") {
      setStatus($("menu-status"), "Updating. You can keep working; this takes a while.");
      setTimeout(pollUpdate, 3000);
    } else if (r.state === "done") {
      setStatus($("menu-status"), "Up to date. A new version, if there was one, starts after the next restart.", "ok");
    } else if (r.state === "failed") {
      setStatus($("menu-status"), "The update failed. Your current version keeps working.", "err");
    }
  }

  document.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") { if (zone === "ribbon") tileIdx = Math.min(tileIdx + 1, APPS.length - 1); else powerIdx = Math.min(powerIdx + 1, powerBtns.length - 1); }
    else if (e.key === "ArrowLeft") { if (zone === "ribbon") tileIdx = Math.max(tileIdx - 1, 0); else powerIdx = Math.max(powerIdx - 1, 0); }
    else if (e.key === "ArrowUp") zone = "power";
    else if (e.key === "ArrowDown") zone = "ribbon";
    else if (e.key === "Enter") { if (zone === "ribbon") openApp(tileIdx); else power(powerBtns[powerIdx].dataset.power); }
    else if (/^[1-9]$/.test(e.key) && Number(e.key) <= APPS.length) { tileIdx = Number(e.key) - 1; zone = "ribbon"; openApp(tileIdx); }
    else return;
    e.preventDefault();
    paint();
  });
  paint();
  pollUpdate();
}

// ---------- Login ----------
function initLogin(state) {
  keyHandler = null;
  show("login");
  const user = $("login-user"), pass = $("login-pass");
  if (state.profiles && state.profiles.length) user.value = state.profiles[0];
  (user.value ? pass : user).focus();
  let sending = false;
  async function submit() {
    if (sending) return;
    if (!user.value) return user.focus();
    sending = true;
    setStatus($("login-status"), "Opening your profile...");
    const r = await api("login", { user: user.value.trim(), password: pass.value });
    sending = false;
    if (r.ok) { setStatus($("login-status"), "Welcome.", "ok"); sending = true; return; }
    pass.value = "";
    pass.focus();
    setStatus($("login-status"), r.error || "Wrong profile name or password.", "err");
  }
  user.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); pass.focus(); } });
  pass.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); submit(); } });
}

// ---------- First run ----------
const body = () => $("setup-body");
function steps(n) {
  [...$("steps").children].forEach((s, i) => s.classList.toggle("on", i < n));
}

// A vertical list moved with the arrow keys, chosen with Enter.
function pickList(items, render, onPick) {
  const list = document.createElement("div");
  list.className = "list";
  let idx = 0;
  items.forEach((it, i) => {
    const r = document.createElement("button");
    r.className = "row";
    r.innerHTML = render(it);
    r.addEventListener("mouseenter", () => { idx = i; paint(); });
    r.addEventListener("click", () => onPick(it));
    list.appendChild(r);
  });
  function paint() {
    [...list.children].forEach((r, i) => r.classList.toggle("focus", i === idx));
    if (list.children[idx]) list.children[idx].scrollIntoView({ block: "nearest" });
  }
  list.onkey = (e) => {
    if (e.key === "ArrowDown") idx = Math.min(idx + 1, items.length - 1);
    else if (e.key === "ArrowUp") idx = Math.max(idx - 1, 0);
    else if (e.key === "Enter") { onPick(items[idx]); return true; }
    else return false;
    paint();
    return true;
  };
  paint();
  return list;
}

let keyHandler = null;
document.addEventListener("keydown", (e) => {
  if (MODE !== "greeter" || !keyHandler) return;
  if (keyHandler(e)) e.preventDefault();
});

function busy(text) {
  body().innerHTML = `<div class="busy"><div class="spinner"></div><span>${text}</span></div>`;
  keyHandler = null;
}
function errorBox(text, retry) {
  body().innerHTML = `<div class="eyebrow">SOMETHING WENT WRONG</div><h1>Setup could not continue</h1>
    <p class="lead"></p><div class="actions"><button class="btn primary" id="retry">Try again</button></div>`;
  body().querySelector("p").textContent = text;
  $("retry").onclick = retry;
  $("retry").focus();
  keyHandler = (e) => { if (e.key === "Enter") { retry(); return true; } return false; };
}

async function setupKeyboard() {
  steps(1);
  busy("Starting...");
  const r = await api("setup/layouts");
  if (r.error) return errorBox(r.error, setupKeyboard);
  body().innerHTML = `<div class="eyebrow">STEP 1 · KEYBOARD</div><h1>Which keyboard do you have?</h1>
    <p class="lead">Pick the layout printed on your keys, so what you type matches what you see.</p>`;
  const list = pickList(r.layouts, (l) => `<span>${l.name}</span><span class="meta">${l.id}</span>`, async (l) => {
    busy("Setting up your keyboard...");
    const s = await api("setup/layout", { id: l.id });
    if (s.error) return errorBox(s.error, setupKeyboard);
    // The screen restarts with the new layout and continues with step 2.
  });
  body().appendChild(list);
  keyHandler = list.onkey;
}

async function setupNetwork() {
  steps(2);
  busy("Checking the internet connection...");
  const r = await api("setup/network");
  if (r.error) return errorBox(r.error, setupNetwork);
  if (r.online) return setupProfile();
  body().innerHTML = `<div class="eyebrow">STEP 2 · INTERNET</div><h1>Connect to Wi-Fi</h1>
    <p class="lead">There is no internet over a cable. Choose your network.</p>`;
  const nets = r.wifi && r.wifi.length ? r.wifi : [];
  if (!nets.length) {
    const p = document.createElement("p");
    p.className = "lead";
    p.textContent = "No Wi-Fi networks found.";
    body().appendChild(p);
  }
  const list = pickList(nets.concat([{ ssid: null }]), (n) => n.ssid === null
    ? `<span>Continue without internet</span><span class="meta">SKIP</span>`
    : `<span>${escapeHtml(n.ssid)}</span><span class="meta">${n.secure ? "LOCKED · " : ""}${n.signal}%</span>`,
    (n) => n.ssid === null ? setupProfile() : (n.secure ? wifiPassword(n) : wifiConnect(n, "")));
  body().appendChild(list);
  keyHandler = list.onkey;
}

function wifiPassword(net) {
  body().innerHTML = `<div class="eyebrow">STEP 2 · INTERNET</div><h1></h1>
    <div class="field"><label for="wifi-pass">WI-FI PASSWORD</label><input id="wifi-pass" type="password"></div>
    <div class="status" id="wifi-status"></div>
    <div class="hints"><span><b>ENTER</b> Connect</span><span><b>ESC</b> Back</span></div>`;
  body().querySelector("h1").textContent = net.ssid;
  const inp = $("wifi-pass");
  inp.focus();
  keyHandler = (e) => {
    if (e.key === "Enter") { wifiConnect(net, inp.value); return true; }
    if (e.key === "Escape") { setupNetwork(); return true; }
    return false;
  };
}

async function wifiConnect(net, password) {
  busy(`Connecting to ${escapeHtml(net.ssid)}...`);
  const r = await api("setup/wifi", { ssid: net.ssid, password });
  if (r.error) {
    if (net.secure) { wifiPassword(net); setStatus($("wifi-status"), r.error, "err"); }
    else errorBox(r.error, setupNetwork);
    return;
  }
  setupProfile();
}

async function setupProfile() {
  steps(3);
  busy("Preparing your profile...");
  const p = await api("setup/proposal");
  if (p.error) return errorBox(p.error, setupProfile);
  body().innerHTML = `<div class="eyebrow">STEP 3 · YOUR PROFILE</div><h1>Write these down on paper</h1>
    <div class="secrets">
      <div class="secret"><span class="k">PROFILE NAME</span><span class="v" id="p-name"></span></div>
      <div class="secret"><span class="k">PASSWORD</span><span class="v" id="p-pass"></span></div>
    </div>
    <p class="warn">They are shown only once. Never photograph them or send them to anyone.
    Your profile is encrypted with this password; without it nobody can open it, not even you.</p>
    <div class="actions">
      <button class="btn primary" id="use">I wrote them down</button>
      <button class="btn" id="own">Choose my own</button>
    </div>`;
  $("p-name").textContent = p.name;
  $("p-pass").textContent = p.password;
  const btns = [$("use"), $("own")];
  let i = 0;
  const paint = () => btns.forEach((b, j) => b.classList.toggle("focus", j === i));
  btns[0].onclick = () => createProfile(p.name, p.password, false);
  btns[1].onclick = () => ownProfile();
  paint();
  keyHandler = (e) => {
    if (e.key === "ArrowRight" || e.key === "Tab") { i = (i + 1) % 2; paint(); return true; }
    if (e.key === "ArrowLeft") { i = (i + 1) % 2; paint(); return true; }
    if (e.key === "Enter") { btns[i].click(); return true; }
    return false;
  };
}

function ownProfile(message) {
  body().innerHTML = `<div class="eyebrow">STEP 3 · YOUR PROFILE</div><h1>Choose your own</h1>
    <div class="field"><label for="o-name">PROFILE NAME · lowercase letters, digits and -</label><input id="o-name" spellcheck="false"></div>
    <div class="field"><label for="o-pass">PASSWORD · at least 12 characters</label><input id="o-pass" type="password"></div>
    <div class="field"><label for="o-pass2">REPEAT PASSWORD</label><input id="o-pass2" type="password"></div>
    <div class="status err" id="o-status"></div>
    <div class="hints"><span><b>ENTER</b> Next / Create</span><span><b>ESC</b> Back to suggested</span></div>`;
  if (message) $("o-status").textContent = message;
  const fields = [$("o-name"), $("o-pass"), $("o-pass2")];
  fields[0].focus();
  keyHandler = (e) => {
    if (e.key === "Escape") { setupProfile(); return true; }
    if (e.key !== "Enter") return false;
    const at = fields.indexOf(document.activeElement);
    if (at >= 0 && at < 2) { fields[at + 1].focus(); return true; }
    const name = fields[0].value.trim(), pw = fields[1].value;
    if (!/^[a-z][a-z0-9-]{1,30}$/.test(name)) { $("o-status").textContent = "Use 2 to 31 lowercase letters, digits or -, starting with a letter."; fields[0].focus(); return true; }
    if (pw.length < 12) { $("o-status").textContent = "The password must be at least 12 characters."; fields[1].focus(); return true; }
    if (pw !== fields[2].value) { $("o-status").textContent = "The passwords do not match."; fields[2].value = ""; fields[2].focus(); return true; }
    createProfile(name, pw, true);
    return true;
  };
}

async function createProfile(name, password, custom) {
  busy("Creating your encrypted profile. This takes a minute...");
  const r = await api("setup/create", { name, password });
  if (r.error) {
    if (custom) return ownProfile(r.error);
    return errorBox(r.error, () => createProfile(name, password, false));
  }
  const f = await api("setup/finish", {});
  if (f.error) return errorBox(f.error, () => createProfile(name, password, custom));
  busy("All set. Starting Chlorum...");
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

// ---------- Start ----------
(async function start() {
  if (MODE === "menu") {
    const info = await api("info");
    initMenu(info);
  } else {
    const state = await api("state");
    if (state.setup) { show("setup"); state.layout_done ? setupNetwork() : setupKeyboard(); }
    else initLogin(state);
  }
})();
