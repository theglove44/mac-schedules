const { invoke } = window.__TAURI__.core;

const GROUP_ORDER = [
  "User Agents",
  "Global Agents",
  "System Daemons",
  "Apple Agents",
  "Apple Daemons",
  "Cron",
];

const state = {
  jobs: [],
  filter: "home",
  search: "",
  selectedId: null,
};

// Stable id for a job (label may repeat across domains).
const jobId = (j) => `${j.source_group}::${j.label}::${j.source_path}`;

// launchd's disabled database wins over the plist's Disabled key. `enable`/
// `disable` write the database, so this is the state the toggle actually sets.
const isDisabled = (j) => (j.disabled_override != null ? j.disabled_override : j.disabled_key);

async function loadJobs() {
  setStatus("Loading…");
  try {
    const [launchd, cron] = await Promise.all([
      invoke("get_launchd_jobs"),
      invoke("get_cron_jobs"),
    ]);
    state.jobs = [...launchd, ...cron];
    render();
    setStatus("");
  } catch (e) {
    reportError("Could not read scheduled jobs", e);
  }
}

function matchesFilter(job) {
  if (state.filter === "all") return true;
  if (state.filter === "apple") return job.scope === "apple";
  return job.source_group === state.filter;
}

function matchesSearch(job) {
  if (!state.search) return true;
  const q = state.search.toLowerCase();
  return (
    job.label.toLowerCase().includes(q) ||
    (job.program || "").toLowerCase().includes(q) ||
    (job.schedule_human || "").toLowerCase().includes(q)
  );
}

function render() {
  const home = document.getElementById("home");
  const list = document.getElementById("listbox");
  const detail = document.getElementById("detail");
  const search = document.getElementById("search");
  const isHome = state.filter === "home";

  home.hidden = !isHome;
  list.hidden = isHome;
  detail.hidden = isHome;
  search.hidden = isHome;

  if (isHome) {
    document.getElementById("status-count").textContent = "Home";
    return;
  }

  list.innerHTML = "";

  const visible = state.jobs.filter((j) => matchesFilter(j) && matchesSearch(j));

  // Group
  const groups = new Map();
  for (const g of GROUP_ORDER) groups.set(g, []);
  for (const j of visible) {
    if (!groups.has(j.source_group)) groups.set(j.source_group, []);
    groups.get(j.source_group).push(j);
  }

  let total = 0;
  for (const [group, items] of groups) {
    if (items.length === 0) continue;
    items.sort((a, b) => a.label.localeCompare(b.label));
    const header = document.createElement("div");
    header.className = "group-header";
    header.textContent = `${group} (${items.length})`;
    list.appendChild(header);

    for (const job of items) {
      list.appendChild(rowFor(job));
      total++;
    }
  }

  if (state.filter === "Cron" && visible.length === 0) {
    const empty = document.createElement("div");
    empty.className = "group-header";
    empty.style.fontWeight = "normal";
    empty.textContent = "No cron jobs installed.";
    list.appendChild(empty);
  }

  document.getElementById("status-count").textContent =
    `${total} job${total === 1 ? "" : "s"}` +
    (state.filter === "all" ? "" : ` in ${state.filter === "apple" ? "Apple" : state.filter}`);

  // Keep selection if still visible
  if (state.selectedId && !visible.some((j) => jobId(j) === state.selectedId)) {
    // selection filtered out; leave detail as-is
  }
}

function rowFor(job) {
  const row = document.createElement("div");
  row.className = "row";
  if (isDisabled(job)) row.classList.add("disabled");
  if (jobId(job) === state.selectedId) row.classList.add("selected");

  const dot = document.createElement("span");
  dot.className = "dot " + dotClass(job);
  dot.title = dotTitle(job);

  const label = document.createElement("span");
  label.className = "row-label";
  label.textContent = job.label;

  row.appendChild(dot);
  row.appendChild(label);
  row.addEventListener("click", () => {
    state.selectedId = jobId(job);
    render();
    renderDetail(job);
  });
  return row;
}

function dotClass(job) {
  if (isDisabled(job)) return "off";
  if (job.pid != null) return "running";
  if (job.loaded) return "loaded";
  return "off";
}
function dotTitle(job) {
  if (isDisabled(job)) return "Disabled";
  if (job.pid != null) return "Running (pid " + job.pid + ")";
  if (job.loaded) return "Loaded, idle";
  return "Not loaded";
}

function renderDetail(job) {
  const el = document.getElementById("detail");
  const enabled = !isDisabled(job);
  const stateBadge = enabled
    ? '<span class="badge on">Enabled</span>'
    : '<span class="badge offb">Disabled</span>';
  const runBadge =
    job.pid != null
      ? `<span class="badge on">Running · pid ${job.pid}</span>`
      : job.loaded
      ? '<span class="badge">Loaded</span>'
      : '<span class="badge">Not loaded</span>';

  const cmd = job.args && job.args.length ? job.args.join(" ") : job.program;

  const rows = [];
  rows.push(["Label", esc(job.label)]);
  rows.push(["Schedule", esc(job.schedule_human)]);
  rows.push(["State", stateBadge + " " + (job.kind === "launchd" ? runBadge : "")]);
  if (cmd) rows.push(["Command", `<code>${esc(cmd)}</code>`]);
  if (job.last_exit != null && job.pid == null)
    rows.push(["Last exit", String(job.last_exit)]);
  rows.push(["Source", `<span class="mono">${esc(job.source_path)}</span>`]);
  if (job.stdout_path) rows.push(["Log (out)", logLink(job.stdout_path)]);
  if (job.stderr_path) rows.push(["Log (err)", logLink(job.stderr_path)]);

  const dl = rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("");

  let actions = "";
  if (job.kind === "launchd") {
    if (job.apple) {
      actions = `<button class="btn" disabled title="Apple system job — protected">${
        enabled ? "Disable" : "Enable"
      }</button><span style="align-self:center;color:#777"> Apple job — protected</span>`;
    } else {
      actions = `<button class="btn default" id="toggle-btn">${
        enabled ? "Disable" : "Enable"
      }</button>`;
      actions += `<button class="btn" id="delete-btn">Delete…</button>`;
    }
    actions += `<button class="btn" id="reveal-btn">Reveal plist</button>`;
  }

  el.innerHTML = `
    <h2>${esc(job.label)}</h2>
    <dl>${dl}</dl>
    <div class="detail-actions">${actions}</div>
  `;

  const toggleBtn = document.getElementById("toggle-btn");
  if (toggleBtn) toggleBtn.addEventListener("click", () => confirmToggle(job));
  const deleteBtn = document.getElementById("delete-btn");
  if (deleteBtn) deleteBtn.addEventListener("click", () => confirmDelete(job));
  const revealBtn = document.getElementById("reveal-btn");
  if (revealBtn)
    revealBtn.addEventListener("click", () =>
      invoke("reveal", { path: job.source_path }).catch((e) => reportError("Could not reveal file", e))
    );
  el.querySelectorAll("[data-log]").forEach((a) =>
    a.addEventListener("click", () =>
      invoke("open_file", { path: a.getAttribute("data-log") }).catch((e) =>
        reportError("Could not open log", e)
      )
    )
  );
}

function logLink(path) {
  return `<a href="#" data-log="${esc(path)}" class="mono">${esc(path)}</a>`;
}

function clearDetail() {
  state.selectedId = null;
  document.getElementById("detail").innerHTML =
    '<div class="detail-empty">Select a job to see its details.</div>';
}

function confirmDelete(job) {
  const needsAuth = job.scope !== "user";
  let msg =
    `Delete “${job.label}”?\n\n` +
    `The job will be unloaded and its file moved to the Trash:\n${job.source_path}`;
  if (needsAuth)
    msg += "\n\nThis file is owned by the system — macOS will ask for your administrator password.";

  showModal(msg, async () => {
    setStatus(`Deleting ${job.label}…`);
    try {
      const res = await invoke("delete_job", {
        label: job.label,
        path: job.source_path,
        scope: job.scope,
      });
      clearDetail();
      await loadJobs();
      setStatus(res);
    } catch (e) {
      reportError(`Could not delete “${job.label}”`, e);
    }
  });
}

function confirmToggle(job) {
  const enable = isDisabled(job); // if currently disabled, we enable
  const verb = enable ? "enable" : "disable";
  const isDaemon = job.scope === "system";
  let msg = `Are you sure you want to ${verb} “${job.label}”?`;
  if (isDaemon)
    msg += "\n\nThis is a system daemon — macOS will ask for your administrator password.";

  showModal(msg, async () => {
    setStatus(`${enable ? "Enabling" : "Disabling"} ${job.label}…`);
    try {
      const res = await invoke("set_enabled", {
        label: job.label,
        path: job.source_path,
        scope: job.scope,
        enable,
      });
      await loadJobs();
      setStatus(res); // after loadJobs — it clears the status line on success
      // Reselect and refresh detail
      const again = state.jobs.find((j) => jobId(j) === state.selectedId);
      if (again) renderDetail(again);
    } catch (e) {
      reportError(`Could not ${verb} “${job.label}”`, e);
    }
  });
}

/* ---- Modal ------------------------------------------------------------ */
let modalOnOk = null;
function showModal(text, onOk) {
  const modal = document.getElementById("modal");
  document.getElementById("modal-text").textContent = text;
  document.getElementById("modal-cancel").hidden = false;
  modalOnOk = onOk;
  modal.hidden = false;
}
// Single-button dialog. The status bar is one short line, so anything the user
// actually needs to read — launchctl errors especially — goes here instead.
function showAlert(text) {
  const modal = document.getElementById("modal");
  document.getElementById("modal-text").textContent = text;
  document.getElementById("modal-cancel").hidden = true;
  modalOnOk = null;
  modal.hidden = false;
}
function hideModal() {
  document.getElementById("modal").hidden = true;
  document.getElementById("modal-cancel").hidden = false;
  modalOnOk = null;
}

function setStatus(msg) {
  const el = document.getElementById("status-msg");
  el.textContent = msg;
  el.title = msg; // full text on hover when the bar truncates
}

function reportError(context, e) {
  const detail = e && e.message ? e.message : String(e);
  setStatus(`${context}: ${detail}`);
  showAlert(`${context}\n\n${detail}`);
}

function esc(s) {
  return String(s == null ? "" : s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/* ---- Wiring ----------------------------------------------------------- */
window.addEventListener("DOMContentLoaded", () => {
  document.getElementById("tabs").addEventListener("click", (e) => {
    const tab = e.target.closest(".tab");
    if (!tab) return;
    document.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
    tab.classList.add("active");
    state.filter = tab.dataset.filter;
    render();
  });

  document.getElementById("search").addEventListener("input", (e) => {
    state.search = e.target.value.trim();
    render();
  });

  document.getElementById("refresh").addEventListener("click", loadJobs);

  document.getElementById("modal-ok").addEventListener("click", () => {
    const cb = modalOnOk;
    hideModal();
    if (cb) cb();
  });
  document.getElementById("modal-cancel").addEventListener("click", hideModal);

  document.getElementById("close-box").addEventListener("click", () => {
    window.__TAURI__.window.getCurrentWindow().close();
  });
  document.getElementById("min-box").addEventListener("click", () => {
    window.__TAURI__.window.getCurrentWindow().minimize();
  });

  loadJobs();
});
