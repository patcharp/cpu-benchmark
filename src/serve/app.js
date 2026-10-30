"use strict";
// All data is inserted with textContent / property assignment, never innerHTML:
// report names and CPU strings come from files and must not be able to inject markup.

const $ = (id) => document.getElementById(id);
function h(tag, props, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === "class") e.className = v;
    else if (k === "text") e.textContent = v;
    else e[k] = v;
  }
  for (const kid of kids) e.append(kid);
  return e;
}
function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }

async function api(path, body) {
  const opt = body === undefined ? {} : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
  const res = await fetch(path, opt);
  let data = {};
  try { data = await res.json(); } catch (_) { /* non-JSON error body */ }
  if (!res.ok) throw new Error(data.error || ("HTTP " + res.status));
  return data;
}

function num(v) {
  if (v === null || v === undefined || !isFinite(v)) return "-";
  const a = Math.abs(v);
  return a >= 1000 ? v.toFixed(0) : a >= 100 ? v.toFixed(1) : a >= 1 ? v.toFixed(2) : v.toFixed(3);
}
function bytes(n) {
  const u = [["GiB", 2 ** 30], ["MiB", 2 ** 20], ["KiB", 1024]];
  for (const [name, size] of u) if (n >= size) return (n / size).toFixed(n / size % 1 ? 1 : 0) + " " + name;
  return n + " B";
}
function when(unix) { return unix ? new Date(unix * 1000).toLocaleString() : "-"; }
function mmss(s) { s = Math.round(s); return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0"); }

let info = null;
let lastState = "";
let selected = []; // report names chosen for compare, in click order

function renderSystem() {
  const s = info.system, b = info.build, dl = $("sys");
  clear(dl);
  const topo = s.sockets ? `${s.sockets} socket, ${s.physical_cores} core, ${s.logical_cpus} logical CPU (${s.threads_per_core} thread/core)` : `${s.logical_cpus} logical CPU`;
  const rows = [
    ["CPU", s.cpu_model], ["Arch", `${s.arch} (${s.cpu_vendor})`], ["Topology", topo],
    ["Memory", bytes(s.mem_total_bytes)], ["OS", `${s.os} / ${s.kernel}`],
    ["Page size", bytes(s.page_size)], ["Governor", s.governor || "unknown"],
    ["Build", `${b.target} (${b.libc}), ${b.rustflags || "default flags"}`],
    ["การเข้าถึง", info.auth ? "ต้องใช้ token" : "ไม่มี token (loopback เท่านั้นโดยค่าเริ่มต้น)"],
  ];
  if (s.virtualization) rows.push(["Virtualization", s.virtualization]);
  if (s.load_avg) rows.push(["Load average", s.load_avg.map((x) => x.toFixed(2)).join(" / ") + " (ตอนเปิดเซิร์ฟเวอร์)"]);
  for (const [k, v] of rows) { dl.append(h("dt", { text: k }), h("dd", { text: String(v) })); }
  $("ver").textContent = "v" + info.version + " · " + info.hostname;
  $("f-threads").placeholder = `ว่าง = ทุก CPU (${s.usable_parallelism})`;
}

function setState(el, state) { el.className = "chip " + state; el.textContent = state; }

function renderStatus(st) {
  setState($("state"), st.state);
  const running = st.state === "running";
  $("startbtn").disabled = running;
  $("busy").textContent = running ? `กำลังรัน '${st.label}' อยู่ — รอให้จบหรือยกเลิกก่อน` : "";
  for (const el of document.querySelectorAll("#runform input, #runform select")) el.disabled = running;

  const show = st.state !== "idle";
  $("prog").classList.toggle("hidden", !show);
  if (show) {
    setState($("pstate"), st.state);
    $("plabel").textContent = st.label;
    const pct = st.total ? Math.round((st.done / st.total) * 100) : 0;
    $("pbar").style.width = (st.state === "done" ? 100 : pct) + "%";
    $("pinfo").textContent = `${st.done}/${st.total} kernel · ${mmss(st.elapsed_secs)}` + (st.current ? ` · กำลังวัด ${st.current}` : "");
    $("cancelbtn").classList.toggle("hidden", !running);
    const perr = $("perr");
    perr.classList.toggle("hidden", !st.error);
    perr.textContent = st.error || "";
    const done = $("pdone");
    clear(done);
    done.classList.toggle("hidden", st.state !== "done");
    if (st.state === "done") {
      done.append(h("a", { href: `/reports/${encodeURIComponent(st.label)}.html`, target: "_blank", rel: "noopener", text: "เปิดรายงาน HTML" }),
        h("a", { href: `/reports/${encodeURIComponent(st.label)}.json`, target: "_blank", rel: "noopener", text: "JSON" }));
    }
    const tb = $("ptable");
    clear(tb);
    const skipped = st.results.filter((r) => r.status === "skipped");
    for (const r of st.results) {
      if (r.status === "skipped") continue; // summarised below instead of flooding the table
      const problem = r.status !== "ok";
      tb.append(h("tr", {}, h("td", { text: r.id }), h("td", { text: r.unit }),
        h("td", { class: "r", text: num(r.single) + (r.cv_single > 5 ? " ⚠" : "") }),
        h("td", { class: "r", text: num(r.multi) + (r.cv_multi > 5 ? " ⚠" : "") }),
        h("td", { text: problem ? r.status + (r.note ? ": " + r.note : "") : "ok" })));
    }
    if (skipped.length) {
      tb.append(h("tr", {}, h("td", { colSpan: 5, class: "muted", text: `ข้าม ${skipped.length} kernel (${skipped[0].note || "ไม่ได้เลือก"})` })));
    }
    $("plog").textContent = st.log_tail || "";
  }
  const intr = $("interrupted");
  intr.classList.toggle("hidden", !st.interrupted.length);
  if (st.interrupted.length) {
    $("intnote").textContent = "พบการรันที่ไม่จบ (เซิร์ฟเวอร์ถูกปิดหรือโปรเซสตาย): " +
      st.interrupted.map((i) => `${i.label} (วัดเสร็จ ${i.kernels_done} kernel)`).join(", ") +
      ". ผลบางส่วนไม่ถูกนับเป็นรายงาน ให้รันใหม่";
  }
  if (lastState === "running" && st.state !== "running") loadReports();
  lastState = st.state;
}

function renderReports(data) {
  const tb = $("rtable");
  clear(tb);
  $("rempty").classList.toggle("hidden", data.reports.length > 0);
  for (const r of data.reports) {
    if (r.error) {
      tb.append(h("tr", {}, h("td"), h("td", { text: r.name }), h("td", { colSpan: 9, text: "อ่านไม่ได้: " + r.error })));
      continue;
    }
    const cb = h("input", { type: "checkbox", checked: selected.includes(r.name) });
    cb.addEventListener("change", () => {
      selected = selected.filter((n) => n !== r.name);
      if (cb.checked) selected.push(r.name);
      if (selected.length > 2) selected = selected.slice(-2);
      renderReports(data);
    });
    const link = (ext) => h("a", { href: `/reports/${encodeURIComponent(r.name)}.${ext}`, target: "_blank", rel: "noopener", text: ext });
    const open = h("td", {}); open.append(link("html"), " ", link("md"), " ", link("json"));
    const pos = selected.indexOf(r.name);
    tb.append(h("tr", {}, h("td", {}, cb, pos >= 0 ? h("span", { text: pos === 0 ? " A" : " B" }) : ""),
      h("td", { text: r.name }), h("td", { text: r.cpu }), h("td", { text: r.arch }),
      h("td", { text: r.profile + (r.profile !== "full" ? " ⚠" : "") }),
      h("td", { class: "r", text: String(r.threads) }),
      h("td", { class: "r", text: r.score_single ? r.score_single.toFixed(0) : "-" }),
      h("td", { class: "r", text: r.score_multi ? r.score_multi.toFixed(0) : "-" }),
      h("td", { text: r.libc }), h("td", { text: when(r.started_unix) }), open));
  }
  $("cmpbtn").disabled = selected.length !== 2;
  $("cmpinfo").textContent = selected.length === 2 ? `A = ${selected[0]}, B = ${selected[1]}` : "เลือก 2 รายงาน";
  const list = $("cmplist");
  clear(list);
  if (data.comparisons.length) {
    list.append(h("div", { class: "muted", text: "การเปรียบเทียบที่เคยสร้าง:" }));
    for (const c of data.comparisons) list.append(h("div", {}, h("a", { href: "/reports/" + encodeURIComponent(c), target: "_blank", rel: "noopener", text: c })));
  }
}

let reportsCache = { reports: [], comparisons: [] };
async function loadReports() {
  try { reportsCache = await api("/api/reports"); renderReports(reportsCache); } catch (e) { /* shown on next poll */ }
}

$("runform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const v = (id) => $(id).value.trim();
  const body = { profile: v("f-profile"), mode: v("f-mode") };
  if (v("f-label")) body.label = v("f-label");
  if (v("f-threads")) body.threads = parseInt(v("f-threads"), 10);
  if (v("f-cpus")) body.cpus = v("f-cpus");
  if (v("f-only")) body.only = v("f-only");
  if (v("f-skip")) body.skip = v("f-skip");
  if (v("f-mem")) body.mem_budget_mb = parseInt(v("f-mem"), 10);
  const box = $("runerr");
  box.classList.add("hidden");
  try { await api("/api/run", body); await poll(); } catch (e) { box.textContent = e.message; box.classList.remove("hidden"); }
});
$("f-profile").addEventListener("change", () => $("notfull").classList.toggle("hidden", $("f-profile").value === "full"));
$("cancelbtn").addEventListener("click", async () => {
  if (!confirm("ยกเลิกการรันนี้? ผลที่วัดไปแล้วจะถูกทิ้ง")) return;
  try { await api("/api/cancel", {}); await poll(); } catch (e) { alert(e.message); }
});
$("cmpbtn").addEventListener("click", async () => {
  const opt = (id) => { const x = parseFloat($(id).value); return x > 0 ? x : undefined; };
  const box = $("cerr");
  box.classList.add("hidden");
  try {
    const r = await api("/api/compare", { a: selected[0], b: selected[1], cost_a: opt("c-ca"), cost_b: opt("c-cb"), watts_a: opt("c-wa"), watts_b: opt("c-wb") });
    window.open(r.html, "_blank", "noopener");
    loadReports();
  } catch (e) { box.textContent = e.message; box.classList.remove("hidden"); }
});

async function poll() {
  try { renderStatus(await api("/api/status")); } catch (e) { setState($("state"), "failed"); $("state").textContent = "เชื่อมต่อไม่ได้"; }
}

(async () => {
  try { info = await api("/api/system"); renderSystem(); } catch (e) { $("sys").textContent = e.message; }
  await Promise.all([poll(), loadReports()]);
  // Poll gently: every request steals a little CPU from a run in progress.
  setInterval(() => poll(), 2500);
  setInterval(() => { if (lastState !== "running") loadReports(); }, 20000);
})();
