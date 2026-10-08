// Corrent web UI: a small client for Corrent's qBittorrent-compatible API (same origin).
// Torrent names are untrusted: everything is rendered with textContent, never as HTML.
"use strict";

const API = "/api/v2";
const $ = (id) => document.getElementById(id);
let timer = null;

async function api(path, { method = "GET", form = null, body = null } = {}) {
  const opts = { method, credentials: "same-origin" };
  if (form) {
    opts.body = new URLSearchParams(form);
  } else if (body) {
    opts.body = body;
  }
  const r = await fetch(API + path, opts);
  if (r.status === 403) {
    showLogin();
    throw new Error("signed out");
  }
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  const type = r.headers.get("content-type") || "";
  return type.includes("json") ? r.json() : r.text();
}

function fmtBytes(n) {
  if (!n || n < 0) return "0 B";
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
}
const fmtSpeed = (n) => (n > 0 ? `${fmtBytes(n)}/s` : "–");

const STATES = {
  downloading: "Downloading", stalledDL: "Stalled", metaDL: "Metadata", checkingDL: "Checking",
  checkingUP: "Checking", uploading: "Seeding", stalledUP: "Seeding", pausedDL: "Paused",
  pausedUP: "Completed", queuedDL: "Queued", queuedUP: "Queued", error: "Error", missingFiles: "Missing files",
};
function stateClass(s) {
  if (s === "error" || s === "missingFiles") return "error";
  if (s === "uploading" || s === "stalledUP") return "seeding";
  if (s === "downloading") return "downloading";
  return "";
}
const isPaused = (s) => s === "pausedDL" || s === "pausedUP";

function toast(text) {
  const t = $("toast");
  t.textContent = text;
  t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (t.hidden = true), 4000);
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function button(label, cls, onClick) {
  const b = el("button", `small ${cls || ""}`, label);
  b.type = "button";
  b.addEventListener("click", onClick);
  return b;
}

async function act(path, hash, extra = {}) {
  try {
    await api(path, { method: "POST", form: { hashes: hash, ...extra } });
    refresh();
  } catch (e) {
    toast(String(e.message || e));
  }
}

function render(torrents) {
  const q = $("search").value.trim().toLowerCase();
  const list = Object.entries(torrents)
    .map(([hash, t]) => ({ hash, ...t }))
    .filter((t) => !q || t.name.toLowerCase().includes(q))
    .sort((a, b) => (a.added_on || 0) - (b.added_on || 0));
  const rows = $("rows");
  rows.replaceChildren();
  for (const t of list) {
    const tr = el("tr");
    const name = el("td", "name", t.name);
    name.title = t.name;
    tr.append(name, el("td", "num", fmtBytes(t.size)));
    const prog = el("td");
    const bar = el("span", t.progress >= 1 ? "bar done" : "bar");
    const fill = el("i");
    fill.style.width = `${Math.min(100, t.progress * 100).toFixed(1)}%`;
    bar.append(fill);
    prog.append(bar, el("span", "pct", `${Math.floor(t.progress * 100)}%`));
    const st = el("td");
    st.append(el("span", `state ${stateClass(t.state)}`, STATES[t.state] || t.state));
    tr.append(prog, st, el("td", "num", fmtSpeed(t.dlspeed)), el("td", "num", fmtSpeed(t.upspeed)),
      el("td", "num", (t.ratio || 0).toFixed(2)));
    const actions = el("td", "actions");
    actions.append(
      isPaused(t.state)
        ? button("Resume", "", () => act("/torrents/resume", t.hash))
        : button("Pause", "", () => act("/torrents/pause", t.hash)),
      button("Remove", "danger", () => {
        if (confirm(`Remove “${t.name}” from Corrent? Downloaded files are kept.`)) {
          act("/torrents/delete", t.hash, { deleteFiles: "false" });
        }
      }),
    );
    tr.append(actions);
    rows.append(tr);
  }
  $("empty").hidden = list.length > 0;
}

function renderCategories(cats) {
  const sel = $("add-category");
  const current = sel.value;
  const names = Object.keys(cats || {}).sort();
  if (sel.options.length - 1 === names.length) return;
  sel.replaceChildren(el("option", "", "No category"));
  sel.options[0].value = "";
  for (const n of names) {
    const o = el("option", "", n);
    o.value = n;
    sel.append(o);
  }
  sel.value = current;
}

async function refresh() {
  try {
    const filter = $("filter").value;
    const [main, list] = await Promise.all([
      api("/sync/maindata"),
      api(`/torrents/info?filter=${encodeURIComponent(filter)}`),
    ]);
    const s = main.server_state || {};
    $("down").textContent = `↓ ${fmtSpeed(s.dl_info_speed)}`;
    $("up").textContent = `↑ ${fmtSpeed(s.up_info_speed)}`;
    renderCategories(main.categories);
    const byHash = {};
    for (const t of list) byHash[t.hash] = t;
    render(byHash);
  } catch (e) {
    if (String(e.message) !== "signed out") toast(`Couldn't refresh: ${e.message || e}`);
  }
}

function showLogin() {
  clearInterval(timer);
  $("app").hidden = true;
  $("login").hidden = false;
  $("user").focus();
}

function showApp() {
  $("login").hidden = true;
  $("app").hidden = false;
  refresh();
  clearInterval(timer);
  timer = setInterval(refresh, 2000);
}

$("login-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const err = $("login-error");
  err.hidden = true;
  try {
    const r = await api("/auth/login", { method: "POST", form: { username: $("user").value, password: $("pass").value } });
    if (r.trim() === "Ok.") {
      $("pass").value = "";
      showApp();
    } else {
      err.textContent = "Wrong username or password.";
      err.hidden = false;
    }
  } catch (e) {
    err.textContent = String(e.message || e);
    err.hidden = false;
  }
});

$("logout").addEventListener("click", async () => {
  try { await api("/auth/logout", { method: "POST" }); } catch (_) { /* signed out anyway */ }
  showLogin();
});

$("add-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const urls = $("add-url").value.trim();
  if (!urls) return;
  try {
    const r = await api("/torrents/add", { method: "POST", form: { urls, category: $("add-category").value } });
    toast(r.trim() === "Ok." ? "Added." : "Corrent couldn't add that link.");
    $("add-url").value = "";
    refresh();
  } catch (e) {
    toast(String(e.message || e));
  }
});

$("add-file").addEventListener("change", async (ev) => {
  const files = [...ev.target.files];
  if (!files.length) return;
  const fd = new FormData();
  for (const f of files) fd.append("torrents", f, f.name);
  fd.append("category", $("add-category").value);
  try {
    await api("/torrents/add", { method: "POST", body: fd });
    toast(files.length === 1 ? "Added 1 torrent." : `Added ${files.length} torrents.`);
    refresh();
  } catch (e) {
    toast(String(e.message || e));
  }
  ev.target.value = "";
});

$("filter").addEventListener("change", refresh);
$("search").addEventListener("input", refresh);

// Signed in already (cookie)? Otherwise show the login form.
api("/app/version").then(showApp).catch(() => showLogin());
