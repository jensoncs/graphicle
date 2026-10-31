"use strict";
const workspaceTitles = {
  overview: "Overview", graph: "Graph", changes: "Changes",
  agents: "Agents & API", usage: "Usage"
};
let workspaceLoad = 0;
function resetProjectViews() {
  workspaceLoad++; window.scrollTo(0, 0);
  state.memories = []; state.sourceOffset = 0;
  for (const id of ["answer-area", "sources-list", "memory-list", "activity-list", "connection-list", "workspace-overview", "workspace-graph", "workspace-changes", "workspace-agents", "workspace-usage"]) {
    const node = document.getElementById(id); if (node) node.replaceChildren();
  }
  $("#detail-dialog").close(); $("#detail-content").replaceChildren(); notice("");
}
function scopeNow() { return {workspace: state.workspace, project: state.project}; }
function sameScope(scope) { return scope.workspace === state.workspace && scope.project === state.project; }
function v1(scope = scopeNow()) { return "/v1/workspaces/" + encodeURIComponent(scope.workspace); }
function projectV1(scope = scopeNow()) { return v1(scope) + "/repos/" + encodeURIComponent(scope.project); }
function scopedCommand(scope, operation, args = {}) {
  if (!sameScope(scope)) throw new Error("The selected project changed. Open this view again.");
  return api(projectV1(scope) + "/commands", {operation, arguments: args});
}
function panel(title, description) {
  const box = el("section", undefined, "panel workspace-panel");
  if (title) box.append(el("h2", title));
  if (description) box.append(el("p", description, "muted"));
  return box;
}
function linkTo(text, href, className = "text-link") {
  const a = el("a", text, className); a.href = href; return a;
}
function metric(value, label, detail) {
  const box = el("div", undefined, "metric");
  box.append(el("span", label, "metric-label"), el("strong", value == null ? "—" : Number(value).toLocaleString(), "metric-value"));
  if (detail) box.append(el("span", detail, "muted"));
  return box;
}
function facts(values) {
  const list = el("dl", undefined, "fact-list");
  for (const [name, value] of values) { list.append(el("dt", name), el("dd", value ?? "Not yet recorded")); }
  return list;
}
function busy(root, message) {
  root.replaceChildren(el("div", message, "loading-state"));
  root.setAttribute("aria-busy", "true");
}
function formatDate(date) { return date ? new Date(date).toLocaleString() : "Not yet recorded"; }
function snippet(root, title, code) {
  const box = panel(title), pre = el("pre", code, "code-sample");
  box.append(pre, button("Copy", async () => {
    await navigator.clipboard.writeText(code); notice("Copied. Keep credentials in your environment.");
  }, "quiet"));
  root.append(box);
}
async function loadWorkspaceView(view) {
  const root = $("#workspace-" + view), scope = scopeNow(), ticket = ++workspaceLoad;
  if (!state.project && view !== "usage") {
    empty(root, "Make room for your next project", "Bring in a repository or source documents. Graphicle connects the evidence, changes and memory in one place.");
    if (can("collect")) root.firstElementChild.append(button("Create your first project", newProject, ""));
    root.firstElementChild.append(linkTo("Read the quickstart", "/docs"));
    return;
  }
  busy(root, "Loading your workspace…");
  const current = () => sameScope(scope) && ticket === workspaceLoad && state.view === view;
  try {
    if (view === "overview") await workspaceOverview(root, scope, current);
    if (view === "graph") await workspaceGraph(root, scope, current);
    if (view === "changes") await workspaceChanges(root, scope, current);
    if (view === "agents") await workspaceAgents(root, scope, current);
    if (view === "usage") await workspaceUsage(root, scope, current);
  } catch (error) {
    if (current()) {
      empty(root, "This view couldn’t load", error.message);
      root.firstElementChild.append(button("Try again", () => loadWorkspaceView(view)));
    }
  } finally { if (current()) root.removeAttribute("aria-busy"); }
}

async function workspaceOverview(root, scope, current) {
  const data = await api(projectV1(scope) + "/overview");
  if (!current()) return;
  root.replaceChildren();
  const status = data.status, graph = data.graph;
  const bar = el("div", undefined, "overview-bar");
  const actions = el("div", undefined, "actions");
  if (state.build?.change_impact) actions.append(button("View changes", () => showView("changes")));
  actions.append(button("Find evidence", () => showView("ask"), ""));
  bar.append(el("p", "Original sources, connected code and memory that stays open to inspection.", "muted"), actions);
  root.append(bar);
  const metrics = el("div", undefined, "metrics");
  metrics.append(metric(status.evidence_sources, "Sources", status.files + " repository files"),
    metric(graph.current?.node_count, "Graph nodes", graph.current_recorded ? "Current snapshot recorded" : "Projection needed"),
    metric(status.memories, "Memories", status.draft_memories + " waiting for review"),
    metric(status.stale_memories, "Need refresh", "Checked against source revisions"));
  root.append(metrics);
  const heading = el("div", undefined, "section-heading");
  heading.append(el("h2", "From source to useful context"), el("span", "Recommended workflow")); root.append(heading);
  const steps = el("div", undefined, "workflow-steps");
  for (const [number, title, description, done, action] of [
    ["01", "Bring your sources", "Sync a repository, upload a document or connect a source.", status.evidence_sources > 0, () => showView("sources")],
    ["02", "Inspect the connections", "Explore code, dependencies and recorded changes.", graph.current_recorded, () => showView("graph")],
    ["03", "Connect your agent", "Use the same evidence through MCP, Python or TypeScript.", null, () => showView("agents")],
    ["04", "Keep the useful parts", "Review memories, follow revisions and refresh stale context.", status.memories > 0 && !status.stale_memories, () => showView("memory")]
  ]) {
    const card = el("button", undefined, "workflow-step");
    card.type = "button"; card.dataset.done = String(done === true); card.addEventListener("click", () => safe(action));
    card.append(el("span", number, "step-number"), el("strong", title), el("span", description),
      el("span", done === null ? "Open setup" : done ? "Explore" : "Get started", "step-action"));
    steps.append(card);
  }
  root.append(steps);
  const columns = el("div", undefined, "workspace-columns"), fresh = panel("Source & memory health", "A result is only as current as its supporting evidence.");
  fresh.append(facts([
    ["Repository updated", formatDate(data.repository.updated_at)],
    ["Source snapshot", (status.source_state || "").slice(0, 16)],
    ["Graph", graph.current_recorded ? "Recorded for the current source snapshot" : "Build a projection from current sources"],
    ["Memory review", status.draft_memories ? status.draft_memories + " drafts need review" : "No pending drafts"],
    ["Memory freshness", status.stale_memories ? status.stale_memories + " need refresh" : "No stale memories in your accessible view"]
  ]));
  if (status.stale_memories || status.draft_memories) fresh.append(button("Review memory", () => showView("memory")));
  const next = panel("Bring Graphicle into your work", "Source search and agent retrieval work without an answering model.");
  next.append(el("pre", "graphicle sync /path/to/project --project " + scope.project, "code-sample"),
    button("Agent connection guide", () => showView("agents")), linkTo("API reference", "/docs"));
  columns.append(fresh, next); root.append(columns);
}

let graphLanding = null;
async function workspaceGraph(root, scope, current) {
  let page = 0;
  const services = async (offset = 0) => {
    const visit = ++page;
    busy(root, "Reading accessible services and repositories…");
    const map = await scopedCommand(scope, "graph.map", {offset, limit: 8});
    if (!current() || visit !== page) return;
    root.replaceChildren();
    root.append(el("h2", "Services & repositories"), el("p", "Start with a repository, choose an entry point, then expand only the dependencies you need.", "muted"));
    root.append(el("p", "Cards represent repository scopes. Service deployment identity and HTTP / gRPC / event wiring are not supplied by this workspace map. Recorded import links below are source dependencies, not proof of service traffic.", "graph-coverage"));
    const cards = el("div", undefined, "graph-projects");
    const choose = async project => {
      if (!current()) return;
      if (project.id === scope.project) return entries();
      if (state.poll) clearTimeout(state.poll); state.poll = null;
      state.project = project.id; $("#project-select").value = project.id;
      resetProjectViews(); graphLanding = project.id;
      return loadWorkspaceView("graph");
    };
    for (const project of map.projects || []) {
      const card = panel(project.name);
      card.append(el("p", project.status === "error" ? project.error :
        Number(project.counts?.files || 0).toLocaleString() + " files · " + Number(project.counts?.symbols || 0).toLocaleString() + " symbols", "muted"));
      card.append(el("p", project.graph?.episode ? (project.graph.current ? "Current recorded graph" : "Graph needs refresh") + " · " + project.graph.episode.slice(0, 12) : "No graph recorded", "graph-path"));
      if (project.status !== "error") card.append(button("Browse entry points", () => choose(project)));
      cards.append(card);
    }
    root.append(cards);
    const names = new Map(state.projects.map(project => [project.id, project.name]));
    const links = panel("Recorded cross-repository dependencies");
    if (!(map.imports || []).length) links.append(el("p", "No import links recorded for this page. This does not establish that these services are independent.", "muted"));
    for (const link of map.imports || []) {
      links.append(el("p", (names.get(link.source_repository_id) || link.source_repository_id) + " → " + (names.get(link.target_repository_id) || link.target_repository_id) + " · " + link.edges + " recorded imports"));
      links.append(el("p", (link.modules || []).join(", "), "graph-path"));
    }
    root.append(links);
    const paging = el("div", undefined, "actions");
    paging.append(el("span", "Repositories " + (map.projects?.length ? offset + 1 : 0) + "–" + (offset + (map.projects?.length || 0)) + " of " + map.total));
    if (offset) paging.append(button("Previous repositories", () => services(Math.max(0, offset - 8)), "quiet"));
    if (map.next_offset != null) paging.append(button("Next repositories", () => services(map.next_offset), "quiet"));
    root.append(paging);
  };
  const entries = async () => {
    if (!current()) return;
    const visit = ++page;
    root.replaceChildren();
    root.append(button("← Services & repositories", () => services(), "quiet"), el("h2", (state.projects.find(project => project.id === scope.project)?.name || "Repository") + " · entry points"));
    const form = el("form", undefined, "graph-controls");
    const query = field(form, "Find entry or source file", "query", {required: false, placeholder: "main, handler, api/…"});
    const kind = field(form, "Entry type", "kind", {choices: [["all", "All types"], ["program_entry_name", "Program entry candidates"], ["http_route", "Recorded HTTP routes"], ["bash_script", "Bash scripts"], ["no_recorded_incoming_calls", "Other call-root candidates"]]});
    const submit = el("button", "Search entries"); form.append(submit); root.append(form);
    const output = el("div"); root.append(output);
    let episode, request = 0, autoKind = true;
    const load = async (offset = 0) => {
      const ticket = ++request;
      busy(output, "Scanning entry points across the recorded graph…");
      try {
        const data = await scopedCommand(scope, "graph.navigate", {tool: "entry_points", query: query.value, kind: kind.value, offset, limit: 50, ...(episode ? {as_of: episode} : {})});
        if (!current() || visit !== page || ticket !== request || !output.isConnected) return;
        episode = data.episode;
        if (autoKind) {
          autoKind = false;
          const kinds = data.counts?.by_kind || {};
          const preferred = ["program_entry_name", "http_route", "bash_script"].find(name => kinds[name] > 0);
          if (preferred) { kind.value = preferred; return load(); }
          // Generic call roots remain opt-in, even when no boundaries were detected.
          kind.value = "http_route";
          return load();
        }
        output.replaceChildren();
        const kinds = data.counts?.by_kind || {};
        output.append(el("p", (kinds.program_entry_name || 0) + " program candidates · " + (kinds.http_route || 0) + " HTTP routes · " + (kinds.bash_script || 0) + " scripts · " + (kinds.no_recorded_incoming_calls || 0) + " other call-root candidates", "muted"));
        output.append(el("p", data.total + " matching entries · " + data.coverage.nodes_scanned.toLocaleString() + " graph nodes scanned · snapshot " + episode.slice(0, 12), "graph-summary"));
        output.append(el("p", "Discovery is incomplete. Declared boundaries are recorded HTTP routes and Bash scripts. Main-named functions and functions with no recorded callers are candidates; missing call edges can create false candidates. Dynamic routes, gRPC and event consumers are not automatically inventoried here.", "graph-coverage"));
        if (data.episode_lag) output.append(el("p", "This graph is older than the current source snapshot. Source links remain pinned to the displayed episode.", "graph-coverage"));
        const groups = new Map();
        for (const entry of data.entries || []) {
          const file = entry.source_file || "No source path recorded";
          if (!groups.has(file)) groups.set(file, []); groups.get(file).push(entry);
        }
        for (const [file, items] of groups) {
          const group = el("details", undefined, "graph-entry-group"); group.open = groups.size <= 6;
          group.append(el("summary", file + " · " + items.length));
          for (const entry of items) {
            const item = row(entry.label, entry.category + " · " + entry.basis.replaceAll("_", " "), entry.source_location || "");
            item.actions.append(button("Explore nested flow", () => tree(entry, episode)));
            if (entry.ambiguous_handler) item.main.append(el("p", "Multiple recorded handlers; no single target selected.", "muted"));
            group.append(item.node);
          }
          output.append(group);
        }
        if (!data.entries.length) output.append(el("p", "No entries match these filters. Unsupported or unresolved entry points may still exist. Use symbol search below.", "muted"));
        const paging = el("div", undefined, "actions");
        if (offset) paging.append(button("Previous entries", () => load(Math.max(0, offset - 50)), "quiet"));
        if (data.next_offset != null) paging.append(button("Next entries", () => load(data.next_offset), "quiet"));
        output.append(paging);
      } finally { if (ticket === request) output.removeAttribute("aria-busy"); }
    };
    form.addEventListener("submit", event => { event.preventDefault(); autoKind = false; safe(() => load()); });
    kind.addEventListener("change", () => { autoKind = false; safe(() => load()); });
    root.append(button("Search all symbols / memory graph", () => { const visit = ++page; return workspaceSymbolGraph(root, scope, () => current() && visit === page); }, "quiet"));
    await load();
  };
  const tree = async (entry, episode) => {
    if (!current()) return;
    const visit = ++page;
    root.replaceChildren();
    const breadcrumb = el("div", undefined, "actions");
    breadcrumb.append(button("Services & repositories", () => services(), "quiet"), el("span", "→"), button("Entry points", entries, "quiet"), el("span", "→ " + entry.label));
    root.append(breadcrumb, el("h2", entry.label), el("p", "Potential static dependencies. Expand one branch at a time; arrows do not prove runtime execution order. Source citations use snapshot " + episode.slice(0, 12) + ".", "muted"));
    const output = el("div", undefined, "graph-tree"); root.append(output);
    let expanded = 0;
    const alive = () => current() && visit === page && output.isConnected;
    const branch = (node, relation, ancestors) => {
      const id = String(node.id || node.node_id), name = node.label || node.name || id;
      const box = el("details", undefined, "graph-tree-branch"), title = el("summary");
      title.append(el("span", name), el("span", relation, "graph-tree-relation")); box.append(title);
      const body = el("div", undefined, "graph-tree-body"); box.append(body);
      let loaded = false, loading = false;
      const load = async () => {
        if (!box.open || loaded || loading || !alive()) return;
        if (ancestors.includes(id)) { loaded = true; body.append(el("p", "Cycle back to an ancestor; expansion stops here.", "muted")); return; }
        if (ancestors.length >= 8 || expanded >= 200) { loaded = true; body.append(el("p", "Traversal limit reached (8 levels / 200 expansions). This branch is incomplete.", "graph-coverage")); return; }
        loading = true; expanded++; busy(body, "Loading recorded dependencies…");
        try {
          const data = await scopedCommand(scope, "graph.as_of", {as_of: episode, node: id, limit: 80, edge_limit: 120,
            relations: ["calls", "may_call", "http_calls", "grpc_calls", "event_delivers", "handles", "invokes"]});
          if (!alive()) return;
          body.replaceChildren(); loaded = true;
          const own = data.nodes.find(item => String(item.id) === id);
          if (own && graphPath(own)) body.append(button("View source · " + graphPath(own), () => openGraphCitation(scope, {relpath: graphPath(own), line: own.line || own.start_line || Number(own.source_location?.match(/\d+/)?.[0]), episode}), "quiet"));
          if (data.truncated || data.edges_truncated) body.append(el("p", "Partial neighborhood: the node or relationship limit was reached.", "graph-coverage"));
          const children = graphChildren(data, id);
          for (const child of children) body.append(branch(child.node, child.relation + (child.count > 1 ? " · " + child.count + " records" : "") + (child.confidence ? " · " + child.confidence : ""), [...ancestors, id]));
          if (!children.length) body.append(el("p", "No outgoing calls or registered handlers recorded in this neighborhood. Unresolved or external dependencies may still exist.", "muted"));
        } catch (error) { if (alive()) { body.replaceChildren(el("p", error.message, "graph-coverage"), button("Retry", load, "quiet")); } }
        finally { loading = false; body.removeAttribute("aria-busy"); }
      };
      box.addEventListener("toggle", () => safe(load));
      return box;
    };
    const first = branch(entry, entry.category + " · " + entry.basis.replaceAll("_", " "), []);
    output.append(first); first.open = true;
  };
  if (graphLanding === scope.project) { graphLanding = null; await entries(); }
  else await services();
}
function graphChildren(data, id) {
  const nodes = new Map((data.nodes || []).map(node => [String(node.id), node])), children = new Map();
  for (const edge of data.edges || []) {
    const relation = graphRelation(edge);
    let target;
    if (relation === "handles" && String(edge.target) === id) target = String(edge.source);
    else if (["calls", "may_call", "http_calls", "grpc_calls", "event_delivers", "invokes"].includes(relation) && String(edge.source) === id) target = String(edge.target);
    else continue;
    const key = JSON.stringify([target, relation, edge.confidence || edge.derivation || ""]);
    if (children.has(key)) { children.get(key).count++; continue; }
    children.set(key, {node: nodes.get(target) || {id: target, label: "Endpoint outside loaded slice: " + target}, relation: relation === "handles" ? "handled by" : relation.replaceAll("_", " "), confidence: edge.confidence || edge.derivation || "", count: 1});
  }
  return Array.from(children.values());
}
async function workspaceSymbolGraph(root, scope, current) {
  const episodes = await scopedCommand(scope, "graph.episodes");
  if (!current()) return;
  root.replaceChildren();
  const form = el("form", undefined, "graph-controls");
  const mode = field(form, "Graph", "mode", {choices: [["code", "Code & dependencies"], ["memory", "Evidence & reviewed memory"]]});
  const episode = field(form, "Snapshot", "episode", {choices: [["latest", "Latest recorded snapshot"], ...episodes.episodes.slice().reverse().map(e => [e.episode, (e.labels?.[0] || e.episode).slice(0, 12) + " · " + new Date(e.created_at).toLocaleDateString()])]});
  const query = field(form, "Find a symbol or source", "query", {required: false, placeholder: "Search names…"});
  const go = el("button", "Explore"); form.append(go); root.append(form);
  const output = el("div"); root.append(output);
  let graphRequest = 0;
  const explore = async node => {
    const request = ++graphRequest;
    const selectedMode = mode.value;
    episode.disabled = selectedMode === "memory";
    if (selectedMode === "code" && !episodes.episodes.length) {
      empty(output, "Build your first code graph", "Sync a repository, then project its current sources to inspect symbols and dependencies.");
      if (can("collect")) output.firstElementChild.append(button("Build graph", async () => {
        const action = output.querySelector("button"); action.disabled = true;
        try { await scopedCommand(scope, "graph.project"); if (current()) await loadWorkspaceView("graph"); }
        finally { if (action.isConnected) action.disabled = false; }
      }, ""));
      return;
    }
    busy(output, "Reading the selected graph…");
    try {
      const value = await scopedCommand(scope, selectedMode === "code" ? "graph.as_of" : "graph.query",
        selectedMode === "code" ? {as_of: episode.value, limit: 80, edge_limit: 120, query: query.value, ...(node ? {node} : {})}
          : {limit: 80, depth: 1, query: query.value, ...(node ? {start: node} : {})});
      if (current() && request === graphRequest) renderGraph(output, value, scope, explore, selectedMode, node);
    } finally { if (request === graphRequest) output.removeAttribute("aria-busy"); }
  };
  form.addEventListener("submit", event => { event.preventDefault(); safe(() => explore()); });
  mode.addEventListener("change", () => safe(() => explore()));
  await explore();
}
function graphLabel(node) { return String(node.label || node.name || node.id); }
function graphPath(node) { return node.source_file || node.file_path || node.path || ""; }
function graphRelation(edge) { return String(edge.relation || edge.kind || "related"); }
// This is a view of returned evidence, never a claim about the entire repository.
function graphFocus(data, requested, relation = "") {
  const nodes = data.nodes || [], edges = (data.edges || []).slice(0, 120);
  const byId = new Map(nodes.map(node => [String(node.id), node]));
  const degree = new Map(nodes.map(node => [String(node.id), 0]));
  for (const edge of edges) for (const id of new Set([String(edge.source), String(edge.target)])) {
    if (degree.has(id)) degree.set(id, degree.get(id) + 1);
  }
  const selected = byId.get(String(requested)) || nodes.slice().sort((a, b) =>
    degree.get(String(b.id)) - degree.get(String(a.id)) || graphLabel(a).localeCompare(graphLabel(b)))[0];
  const id = selected && String(selected.id);
  const matching = edges.filter(edge => !relation || graphRelation(edge) === relation);
  return {
    nodes, edges, byId, selected,
    incoming: matching.filter(edge => String(edge.target) === id && String(edge.source) !== id),
    outgoing: matching.filter(edge => String(edge.source) === id),
    groups: Array.from(nodes.reduce((groups, node) => {
      const path = graphPath(node) || "No source path recorded";
      if (!groups.has(path)) groups.set(path, []);
      groups.get(path).push(node); return groups;
    }, new Map())).sort(([a], [b]) => a.localeCompare(b)),
    partial: !!data.truncated || !!data.edges_truncated || (data.edges || []).length > edges.length
  };
}
function renderGraph(root, data, scope, explore, mode, selectedId) {
  root.replaceChildren();
  const model = graphFocus(data, selectedId), {nodes, edges, selected} = model;
  const summary = el("div", undefined, "graph-summary");
  const total = Number.isFinite(data.node_count) ? " of " + Number(data.node_count).toLocaleString() : "";
  summary.append(el("span", nodes.length + total + " nodes loaded · " + edges.length + " relationships loaded"),
    tag(model.partial ? "partial view" : "selected view"));
  root.append(summary, el("p", mode === "code"
    ? "Static relationships, not observed execution. Search for a handler or function, focus it, then load its neighborhood. Missing connections in this slice do not mean there are no dependencies."
    : "Recorded evidence and memory relationships. Focus an item to inspect the direction and basis of each connection.", "muted"));
  if (model.partial) root.append(el("p", "The response is limited to 80 nodes and 120 relationships. Counts and file groups below describe only this returned slice.", "graph-coverage"));
  if (!selected) { empty(root, "No nodes matched", "Try a different name, snapshot or graph."); return; }
  const layout = el("div", undefined, "graph-layout"), stage = el("section", undefined, "graph-stage"), list = el("nav", undefined, "graph-node-list");
  list.setAttribute("aria-label", "Symbols grouped by source in the returned slice");
  const focus = node => {
    if (!sameScope(scope)) return;
    renderGraph(root, data, scope, explore, mode, String(node.id));
    root.querySelector(".graph-focus-heading").focus();
  };
  const sourceButton = node => {
    const path = graphPath(node);
    if (typeof path === "string" && path && mode === "code") return button("View source", () => openGraphCitation(scope, {
      relpath: path, line: node.line || node.start_line || Number(node.source_location?.match(/\d+/)?.[0]), episode: data.episode
    }), "quiet");
    if (mode === "memory" && String(node.id).startsWith("file:")) return button("View source", () => {
      if (sameScope(scope)) return openSource(node.id);
    }, "quiet");
  };
  const heading = el("h3", graphLabel(selected), "graph-focus-heading"); heading.tabIndex = -1;
  stage.append(el("p", "Focused item · returned slice", "graph-eyebrow"), heading,
    el("p", graphPath(selected) || "No source path recorded", "graph-path"));
  const actions = el("div", undefined, "actions graph-focus-actions");
  actions.append(button("Load neighborhood", () => explore(String(selected.id))), button("Back to search results", () => explore(), "quiet"));
  const source = sourceButton(selected); if (source) actions.append(source);
  stage.append(actions);
  const controls = el("div", undefined, "graph-filter");
  const relation = field(controls, "Relationship", "relation", {choices: [["", "All relationship types"],
    ...Array.from(new Set(edges.map(graphRelation))).sort().map(name => [name, name.replaceAll("_", " ")])]});
  stage.append(controls);
  const board = el("div", undefined, "graph-flow"); stage.append(board);
  const draw = () => {
    board.replaceChildren();
    const view = graphFocus(data, selected.id, relation.value);
    const column = (title, records, direction) => {
      const col = el("section", undefined, "graph-flow-column");
      col.append(el("h4", title + " · " + records.length));
      if (!records.length) col.append(el("p", "None returned for this item and filter. Load its neighborhood to inspect more evidence.", "muted"));
      let expanded = false;
      const content = el("div"); col.append(content);
      const show = () => {
        content.replaceChildren();
        for (const edge of records.slice(0, expanded ? records.length : 6)) {
          const otherId = String(direction === "incoming" ? edge.source : edge.target);
          const other = view.byId.get(otherId);
          const card = el("article", undefined, "graph-relationship");
          card.append(el("p", graphRelation(edge).replaceAll("_", " "), "graph-relation"));
          const from = view.byId.get(String(edge.source)), to = view.byId.get(String(edge.target));
          card.append(el("p", (from ? graphLabel(from) : String(edge.source)) + " → " + (to ? graphLabel(to) : String(edge.target)), "graph-connection"));
          card.append(el("p", other ? graphPath(other) || "No source path recorded" : "Endpoint outside the loaded slice", "graph-path"));
          const basis = [edge.confidence, edge.derivation, edge.evidence].filter(value => typeof value === "string" && value);
          card.append(el("p", basis.length ? "Evidence: " + Array.from(new Set(basis)).join(" · ") : "Evidence basis not recorded", "graph-evidence"));
          if (other) {
            const links = el("div", undefined, "actions");
            links.append(button("Focus " + graphLabel(other), () => focus(other), "quiet"));
            const source = sourceButton(other); if (source) links.append(source);
            card.append(links);
          }
          content.append(card);
        }
        if (records.length > 6) content.append(button(expanded ? "Show fewer" : "Show all " + records.length + " loaded relationships", () => { expanded = !expanded; show(); }, "quiet"));
      };
      show(); return col;
    };
    const center = el("section", undefined, "graph-flow-selected");
    center.append(el("h4", "Selected item"), el("strong", graphLabel(selected)),
      el("p", selected.kind || selected.type || "node", "muted"), el("p", "Incoming → this item → outgoing", "graph-direction"));
    board.append(column("Incoming", view.incoming, "incoming"), center, column("Outgoing", view.outgoing, "outgoing"));
    const incident = new Set([...view.incoming, ...view.outgoing]).size;
    stage.querySelector(".graph-caption").textContent = incident + " loaded relationships match this focus and filter. Other relationships in the returned slice are not displayed here. Arrows follow recorded edge direction; they do not imply execution order.";
  };
  stage.append(el("p", "", "graph-caption"));
  relation.addEventListener("change", draw); draw();
  list.append(el("h3", "Browse loaded sources"), el("p", "Expand a file, then choose a symbol to focus.", "muted"));
  for (const [path, members] of model.groups) {
    const group = el("details", undefined, "graph-source-group");
    group.open = members.some(node => String(node.id) === String(selected.id));
    group.append(el("summary", path + " · " + members.length));
    for (const node of members.slice().sort((a, b) => graphLabel(a).localeCompare(graphLabel(b)))) {
      const item = button(graphLabel(node), () => focus(node), "quiet graph-symbol-choice");
      item.setAttribute("aria-pressed", String(String(node.id) === String(selected.id)));
      group.append(item);
    }
    list.append(group);
  }
  layout.append(stage, list); root.append(layout);
}
async function openGraphCitation(scope, citation) {
  if (!sameScope(scope)) return;
  if (citation.repo && ![scope.project, state.projects.find(p => p.id === scope.project)?.name].includes(citation.repo)) {
    throw new Error("This citation belongs to another project. Select that project to inspect its source.");
  }
  if (!citation.relpath) throw new Error("This graph item has no source location.");
  const source = await scopedCommand(scope, citation.episode ? "graph.source" : "read", {
    path: citation.relpath, start_line: Math.max(1, Number(citation.line) || 1), line_count: 90,
    ...(citation.episode ? {episode: citation.episode} : {})
  });
  if (!sameScope(scope)) return;
  const root = dialog(citation.relpath);
  root.append(el("div", "Lines " + source.line + "–" + source.end_line + " · " + source.sha256.slice(0, 16), "detail-meta"), el("pre", source.quote));
}

async function workspaceChanges(root, scope, current) {
  if (!state.build?.change_impact) {
    // build.info: this edition has no change-impact reports.
    empty(root, "Change impact is not part of Graphicle Core", "Recorded graph history stays available in the Graph view.");
    root.firstElementChild.append(button("Explore the current graph", () => showView("graph")));
    return;
  }
  const history = await scopedCommand(scope, "graph.episodes");
  if (!current()) return;
  root.replaceChildren();
  const recorded = history.episodes.slice().reverse();
  if (recorded.length < 2) {
    empty(root, "Every change needs a before and after", "Sync and project two revisions, or use the CLI to compare commits. Impact reports cite the exact recorded versions.");
    snippet(root, "Compare two local commits", "graphicle pr --repo /path/to/project --base main~1 --head main");
    root.append(button("Explore the current graph", () => showView("graph"))); return;
  }
  const form = el("form", undefined, "graph-controls"), choices = recorded.map(e => [e.episode, (e.labels?.[0] || e.episode).slice(0, 12) + " · snapshot " + e.sequence]);
  const base = field(form, "Before", "base", {choices, value: choices[1][0]});
  const head = field(form, "After", "head", {choices, value: choices[0][0]});
  const run = el("button", "Analyze change"); form.append(run);
  const output = el("div"); root.append(form, output);
  empty(output, "See the reach of a change", "Compare symbols, affected code, linked tests and related documents. Tests suggested by static analysis are distinguished from recorded execution coverage.");
  form.addEventListener("submit", event => { event.preventDefault(); safe(async () => {
    if (base.value === head.value) throw new Error("Choose two different snapshots.");
    run.disabled = true; busy(output, "Tracing the change through your graph…");
    try {
      const result = await scopedCommand(scope, "impact.report", {base: base.value, head: head.value, budget_tokens: 10000});
      if (!current()) return;
      output.replaceChildren();
      const summary = result.summary || {}, metrics = el("div", undefined, "metrics");
      metrics.append(metric(summary.modified_nodes, "Modified items"), metric(summary.added_nodes, "Added items"), metric(result.affected?.length, "Dependents shown"), metric(result.tests?.length, "Test entries shown"));
      output.append(metrics);
      if (result.omitted && Object.values(result.omitted).some(v => typeof v === "number" && v > 0)) output.append(el("p", "This report omits items to stay within its response budget. Download the report to inspect its counts and omission details.", "muted"));
      for (const [label, items] of [["Changed code", result.changed_nodes], ["Affected code", result.affected], ["Tests to investigate", result.tests], ["Related documents", result.documents], ["Wider dependencies", result.wider], ["Unresolved dependencies", result.unresolved]]) {
        if (!items?.length) continue;
        const box = panel(label);
        for (const item of items.slice(0, 40)) {
          const evidenceNames = {executed:"Recorded execution",direct:"Direct dependency",module:"Shared module",name:"Name match",community:"Shared graph group",syntactic:"Code structure",inferred:"Inferred connection","text-match":"Source mention","graph+text-match":"Code and source match",graph:"Code structure"};
          const evidence = [...new Set([item.change, item.evidence, item.via_relation, item.confidence].filter(v => typeof v === "string").map(v => evidenceNames[v] || v.replaceAll("_", " ")))].join(" · ");
          const entry = row(item.label || item.title || item.path || item.relpath || item.citation?.relpath || item.node_id || item.target || "Dependency", evidence, item.citation?.relpath);
          if (item.citation?.relpath) entry.actions.append(button("Read cited source", () => openGraphCitation(scope, item.citation)));
          box.append(entry.node);
        }
        if (items.length > 40) box.append(el("p", "Showing the first 40 items; the download contains the full returned report.", "muted"));
        output.append(box);
      }
      output.append(el("p", "Coverage: " + (result.coverage_status || "No execution coverage uploaded") + ". Dependency links describe possible impact; they do not prove that tests passed.", "muted"));
      output.append(button("Download report JSON", () => downloadJSON(result, "graphicle-impact.json")));
    } catch (error) { if (current()) empty(output, "Couldn’t analyze this change", error.message); }
    finally { run.disabled = false; output.removeAttribute("aria-busy"); }
  }); });
}
function downloadJSON(value, filename) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], {type: "application/json"}));
  const link = el("a"); link.href = url; link.download = filename; link.hidden = true;
  document.body.append(link); link.click(); link.remove();
  notice("Download requested: " + filename);
  setTimeout(() => URL.revokeObjectURL(url), 10000);
}

async function workspaceAgents(root, scope, current) {
  if (!current()) return; root.replaceChildren();
  const intro = panel("One memory, across your tools", "Connect your existing agent to this project. Search returns original evidence and reviewed memory; answering models remain your choice.");
  intro.append(linkTo("Open API & SDK documentation", "/docs")); root.append(intro);
  const columns = el("div", undefined, "workspace-columns"); root.append(columns);
  // The store's folder, when it is not the default one (local apps only).
  let local = {};
  try { local = (await api("/api/workspaces/" + encodeURIComponent(scope.workspace))).local_mcp || {}; } catch (_) { local = {}; }
  if (!current()) return;
  const dataDir = local.data_dir ? ["--data-dir", local.data_dir] : [];
  snippet(columns, "Local MCP connection", JSON.stringify({mcpServers: {graphicle: {command: "graphicle", args: [...dataDir, "mcp", "--project", scope.project]}}}, null, 2));
  snippet(columns, "Remote MCP endpoint", location.origin + "/mcp/" + encodeURIComponent(scope.workspace) + "/" + encodeURIComponent(scope.project) + "\n\nHeader: X-Workspace-Capability: <agent access key>");
  const check = panel("Check the service connection", "Verifies authentication, MCP initialization, tool discovery and project access from this browser. Your external agent still needs its own connection.");
  const results = el("div", undefined, "connection-checks");
  const test = button("Run connection checks", async () => {
    test.disabled = true; results.replaceChildren();
    const endpoint = "/mcp/" + encodeURIComponent(scope.workspace) + "/" + encodeURIComponent(scope.project);
    try {
      for (const [label, method, params] of [
        ["MCP initialized", "initialize", {protocolVersion: "2025-11-25", capabilities: {}, clientInfo: {name: "Graphicle workspace check", version: "1"}}],
        ["Tool list available", "tools/list", {}],
        ["Project access verified", "tools/call", {name: "repository_status", arguments: {}}]
      ]) {
        if (!sameScope(scope)) return;
        const result = await api(endpoint, {jsonrpc: "2.0", id: crypto.randomUUID(), method, params});
        if (result.error || result.result?.isError) throw new Error(result.error?.message || result.result?.content?.[0]?.text || "MCP check failed");
        if (current()) results.append(el("p", label, "check-pass"));
      }
      if (current()) results.append(el("p", "Service checks passed. Connect your agent using the settings above.", "muted"));
    } catch (error) { if (current()) results.append(el("p", error.message, "error")); }
    finally { test.disabled = false; }
  }, "");
  check.append(test, results); root.append(check);
  if (can("admin")) {
    const keys = panel("Agent access keys", "Give each agent its own key. Revoking a key immediately removes its access and cancels its queued work.");
    keys.append(button("Create a scoped agent key", () => $("#manage-access").click(), "secondary"));
    root.append(keys);
  }
  snippet(root, "TypeScript / JavaScript", 'import { Graphicle } from "@graphicle/sdk";\n\nconst client = new Graphicle({\n  url: ' + JSON.stringify(location.origin) + ',\n  workspace: ' + JSON.stringify(scope.workspace) + ',\n  capability: process.env.GRAPHICLE_KEY\n});\nconst project = client.project(' + JSON.stringify(scope.project) + ');\nconst context = await project.context("What depends on our auth module?");\nconsole.log(context.evidence);');
  snippet(root, "Python", 'from graphicle_sdk import Client\nimport os\n\nclient = Client(' + JSON.stringify(location.origin) + ',\n    workspace=' + JSON.stringify(scope.workspace) + ', capability=os.environ["GRAPHICLE_KEY"])\nproject = client.find_project(' + JSON.stringify(state.projects.find(p => p.id === scope.project)?.project_key || scope.project) + ')\npacket = project.context("What depends on our auth module?")\nprint(packet["evidence"])');
}

async function workspaceUsage(root, scope, current) {
  if (!can("admin")) { empty(root, "Your workspace owner manages usage", "Ask the owner to review request budgets and provider usage."); return; }
  const data = await api(v1(scope) + "/usage");
  if (!current()) return; root.replaceChildren();
  const metrics = el("div", undefined, "metrics");
  metrics.append(metric(data.requests, "HTTP requests", data.period + " · UTC"),
    metric(data.remaining, "Requests remaining", data.effective_limit == null ? "No monthly cap" : "Effective limit: " + data.effective_limit.toLocaleString()),
    metric(data.model_usage?.calls, "Model calls", "Uncached, recorded calls"),
    metric((data.model_usage?.input_tokens || 0) + (data.model_usage?.output_tokens || 0), "Model tokens", "Recorded input + output"));
  root.append(metrics);
  const chart = panel("Request activity", "Authenticated HTTP calls across this workspace. Sign-in and recovery operations remain available at the limit.");
  const bars = el("div", undefined, "usage-bars");
  const max = Math.max(1, ...data.daily.map(d => d.requests));
  for (const entry of data.daily) {
    const column = el("div", undefined, "usage-column"), meter = el("meter");
    meter.min = 0; meter.max = max; meter.value = entry.requests;
    meter.setAttribute("aria-label", entry.day + ": " + entry.requests + " requests");
    column.append(el("span", entry.requests.toLocaleString()), meter, el("small", entry.day.slice(5))); bars.append(column);
  }
  if (!data.daily.length) bars.append(el("p", "Your request activity will appear after using the workspace.", "muted"));
  chart.append(bars); root.append(chart);
  const columns = el("div", undefined, "workspace-columns"), budget = panel("Set a monthly request budget", "Cap the authenticated HTTP requests of this workspace each UTC month. Use 0 to pause metered requests.");
  const form = el("form");
  const input = field(form, "Maximum requests per UTC month", "monthly_requests", {required: false, value: data.budget ?? "", placeholder: "No additional cap"});
  input.type = "number"; input.min = "0"; input.max = "1000000000"; input.step = "1";
  const save = el("button", "Save budget"); form.append(save);
  form.addEventListener("submit", event => { event.preventDefault(); safe(async () => {
    save.disabled = true;
    try {
      await api(v1(scope) + "/budget", {monthly_requests: input.value === "" ? null : Number(input.value)});
      notice("Request budget saved."); if (current()) await loadWorkspaceView("usage");
    } finally { save.disabled = false; }
  }); });
  budget.append(form);
  const usage = panel("Provider usage", data.cost_note);
  usage.append(facts([["Recorded input tokens", (data.model_usage?.input_tokens || 0).toLocaleString()], ["Recorded output tokens", (data.model_usage?.output_tokens || 0).toLocaleString()], ["Unreported reservations", data.unreported_reservations || 0], ["HTTP errors", data.http_errors]]));
  columns.append(budget, usage); root.append(columns);
  // What Graphicle returned to agents and keys this month: tokens, not cost.
  const served = data.served || {}, channels = {mcp: "MCP tool results", http: "API", web: "Web app", cli: "Command line", hook: "Session-start context"};
  const servedPanel = panel("What Graphicle served", served.scope);
  servedPanel.append(facts([["Tokens served", Number(served.tokens || 0).toLocaleString()], ["Results served", Number(served.calls || 0).toLocaleString()],
    ...(served.unrecorded_tokens ? [["Counted, not yet recorded", Number(served.unrecorded_tokens).toLocaleString() + " tokens"]] : []),
    ...(served.by_channel || []).map(row => [channels[row.channel] || row.channel, Number(row.tokens).toLocaleString() + " tokens"]),
    ...(served.by_credential || []).slice(0, 5).map(row => [row.label || row.credential_id, Number(row.tokens).toLocaleString() + " tokens"])]));
  root.append(servedPanel);
  // An installed extension can add to this view (/extensions.js).
  if (typeof loadUsageDetails === "function") {
    await loadUsageDetails(root, scope, current, data);
    if (!current()) return;
  }
  // An installed edition can add its own panels here (/extensions.js).
  if (typeof loadPlan === "function") {
    await loadPlan(root, scope, current, data);
    if (!current()) return;
  }
  const recovery = panel("Export & recovery", "Your request budget does not block memory export or deletion receipts. Portable source backups are available through the CLI.");
  if (scope.project) {
    recovery.append(button("Export project memories", async () => downloadJSON(await scopedCommand(scope, "memory.export"), "graphicle-memories.json")));
    recovery.append(button("Inspect deletion receipts", async () => {
      const value = await scopedCommand(scope, "deletion.receipts");
      if (!sameScope(scope)) return;
      const pane = dialog("Deletion receipts");
      pane.append(el("p", "Receipts describe the stores checked. Independently held backups and external model weights are outside this contract.", "muted"), el("pre", JSON.stringify(value, null, 2)));
    }));
  }
  recovery.append(el("pre", "graphicle backup /secure/path/graphicle-backup.sqlite3", "code-sample"));
  root.append(recovery);
  chart.append(el("p", data.scope, "muted usage-note"), button("Download usage JSON", () => downloadJSON(data, "graphicle-usage-" + data.period + ".json"), "quiet"));
}
