"use strict";
const $ = (id) => document.getElementById(id);
let selected = null, timer = null;

function el(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k === "class") e.className = v; else if (k === "text") e.textContent = v; else e.setAttribute(k, v);
  }
  for (const k of kids) e.append(k);
  return e;
}
const usd = (x) => "$" + (x || 0).toFixed(4);
const state = (j) => (j.status && j.status.state) || "queued";
const active = (j) => state(j) === "queued" || state(j) === "running";

async function api(path, opts) {
  const r = await fetch(path, opts);
  let body = null;
  try { body = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error((body && body.error) || r.statusText);
  return body;
}

async function loadHealth() {
  try {
    const h = await api("/api/health");
    const box = $("health");
    box.replaceChildren(el("span", { class: "pill " + (h.api_key_configured ? "ok" : "bad"), text: h.api_key_configured ? "API key set" : "API key missing" }));
    for (const t of h.tools) box.append(el("span", { class: "pill " + (t.available ? "ok" : "no"), title: t.available ? t.purpose : t.install_hint, text: t.name }));
    $("keywarn").hidden = h.api_key_configured;
  } catch (e) { $("health").textContent = "server unreachable"; }
}

async function loadJobs() {
  const list = await api("/api/jobs");
  const ul = $("jobs");
  ul.replaceChildren();
  if (!list.length) ul.append(el("li", { class: "muted", text: "No jobs yet." }));
  for (const j of list) {
    const s = j.status.state;
    const li = el("li", { class: j.id === selected ? "sel" : "" },
      el("div", {}, el("span", { class: "badge " + s, text: s }), (j.instructions || "").slice(0, 70)),
      el("div", { class: "meta", text: new Date(j.created_at).toLocaleString() + " - " + j.input_name + (j.total_cost_usd != null ? " - " + usd(j.total_cost_usd) : "") }));
    li.onclick = () => select(j.id);
    ul.append(li);
  }
}

function eventLine(ev) {
  switch (ev.event) {
    case "planned": return "PLAN  " + ev.plan.summary;
    case "step_started": return "START " + ev.label + (ev.model ? " [" + ev.model + "]" : "");
    case "step_finished": return "DONE  " + ev.report.label + " (" + ev.report.seconds.toFixed(1) + "s, " + usd(ev.report.estimated_cost_usd) + ")";
    case "warning": return "WARN  " + ev.message;
    case "artifact": return "FILE  " + ev.artifact.name + " - " + ev.artifact.description;
    case "log": return "      " + ev.message;
    default: return JSON.stringify(ev);
  }
}

function fileUrl(id, name) { return "/api/jobs/" + id + "/files/" + encodeURIComponent(name); }

function render(j) {
  const d = $("detail");
  const s = state(j);
  const r = j.result;
  d.replaceChildren(el("div", { class: "head" },
    el("div", {}, el("span", { class: "badge " + s, text: s }), el("strong", { text: j.input_name }),
      el("div", { class: "muted", text: j.instructions })),
    (() => { const b = el("button", { class: "link", text: "Delete" }); b.disabled = active(j);
      b.onclick = async () => { if (!confirm("Delete this job?")) return; await api("/api/jobs/" + j.id, { method: "DELETE" }); selected = null; d.textContent = ""; loadJobs(); };
      return b; })()));
  if (s === "failed") d.append(el("div", { class: "err", text: "Failed: " + j.status.error }));
  const plan = r ? r.plan : (j.events.find((e) => e.event === "planned") || {}).plan;
  if (plan) {
    d.append(el("h3", { text: "Plan" }), el("div", { text: plan.summary }));
    const ul = el("ul");
    for (const st of plan.steps) ul.append(el("li", { text: st.kind + " " + JSON.stringify(Object.fromEntries(Object.entries(st).filter(([k, v]) => k !== "kind" && v != null))) }));
    d.append(ul);
    if (plan.deliverables.length) d.append(el("div", { class: "muted", text: "Deliverables: " + plan.deliverables.join(", ") }));
    for (const u of plan.unsupported || []) d.append(el("div", { class: "err", text: "Unsupported: " + u }));
  }
  if (r) {
    d.append(el("h3", { text: "Artifacts" }));
    const ul = el("ul");
    for (const a of r.artifacts) ul.append(el("li", {}, el("a", { href: fileUrl(j.id, a.name), text: a.name }), " " + a.description));
    d.append(r.artifacts.length ? ul : el("div", { class: "muted", text: "none" }));
    for (const w of r.warnings) d.append(el("div", { class: "err", text: "Warning: " + w }));
    d.append(el("h3", { text: "Cost: " + usd(r.total_cost_usd) + " (estimated)" }));
    const t = el("table", {}, el("tr", {}, ...["Step", "Model", "In tok", "Out tok", "Est. USD", "Sec"].map((h, i) => el("th", { class: i > 1 ? "n" : "", text: h }))));
    for (const x of r.steps) t.append(el("tr", {}, el("td", { text: x.label }), el("td", { text: x.model || "-" }),
      ...[x.input_tokens, x.output_tokens, usd(x.estimated_cost_usd), x.seconds.toFixed(1)].map((v) => el("td", { class: "n", text: String(v) }))));
    d.append(t);
    if (r.summary) d.append(el("h3", { text: "Summary" }), el("pre", { text: r.summary }));
  }
  d.append(el("h3", { text: "Log" }), el("pre", { class: "log", text: j.events.map(eventLine).join("\n") || "(no events yet)" }));
}

async function select(id) {
  selected = id;
  clearTimeout(timer);
  loadJobs();
  await refresh();
}

async function refresh() {
  if (!selected) return;
  const id = selected;
  try {
    const j = await api("/api/jobs/" + id);
    if (id !== selected) return;
    render(j);
    if (active(j)) timer = setTimeout(async () => { await refresh(); loadJobs(); }, 2000);
  } catch (e) { $("detail").textContent = e.message; }
}

$("form").onsubmit = async (ev) => {
  ev.preventDefault();
  $("formerr").textContent = "";
  const fd = new FormData();
  fd.append("file", $("file").files[0]);
  fd.append("instructions", $("instructions").value);
  if ($("review").checked) fd.append("review", "on");
  $("submit").disabled = true;
  try {
    const { id } = await api("/api/jobs", { method: "POST", body: fd });
    $("instructions").value = ""; $("file").value = "";
    await select(id);
  } catch (e) { $("formerr").textContent = e.message; }
  $("submit").disabled = false;
};

loadHealth();
loadJobs();
