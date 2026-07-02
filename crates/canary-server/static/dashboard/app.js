(() => {
  const KEY_STORAGE = "canary.dashboard.sessionKey";
  const MODE_STORAGE = "canary.dashboard.mode";
  const VIEWS = ["health", "incidents", "checks", "errors"];

  const state = {
    view: "health",
    key: sessionStorage.getItem(KEY_STORAGE) || "",
    mode: sessionStorage.getItem(MODE_STORAGE) || "",
    loading: false,
    error: "",
    lastRefresh: null,
    selectedIncidentId: "",
    selectedTargetId: "",
    data: {
      status: null,
      health: null,
      incidents: null,
      timeline: null,
      errors: null,
      targets: null,
      monitors: null,
      incidentDetail: null,
      targetChecks: null,
    },
    optional: {
      errorsProblem: "",
      timelineProblem: "",
      adminProblem: "",
    },
  };

  const root = document.getElementById("view-root");
  const keyInput = document.getElementById("api-key");
  const connection = document.getElementById("connection-status");
  const lastRefresh = document.getElementById("last-refresh");

  function init() {
    keyInput.value = state.key;
    setMode(state.mode || preferredMode());
    bindEvents();
    syncViewFromHash();
    render();
    if (state.key) {
      refresh();
    }
  }

  function bindEvents() {
    document.getElementById("session-form").addEventListener("submit", (event) => {
      event.preventDefault();
      state.key = keyInput.value.trim();
      if (state.key) {
        sessionStorage.setItem(KEY_STORAGE, state.key);
      }
      refresh();
    });

    document.getElementById("clear-key").addEventListener("click", () => {
      state.key = "";
      keyInput.value = "";
      sessionStorage.removeItem(KEY_STORAGE);
      state.data = {
        status: null,
        health: null,
        incidents: null,
        timeline: null,
        errors: null,
        targets: null,
        monitors: null,
        incidentDetail: null,
        targetChecks: null,
      };
      state.error = "";
      render();
    });

    document.getElementById("refresh").addEventListener("click", () => refresh());
    document.getElementById("mode-toggle").addEventListener("click", () => {
      setMode(document.documentElement.dataset.aeMode === "dark" ? "light" : "dark");
    });

    document.addEventListener("click", (event) => {
      const viewButton = event.target.closest("[data-view]");
      if (viewButton) {
        event.preventDefault();
        setView(viewButton.dataset.view);
        return;
      }

      const incidentButton = event.target.closest("[data-incident-id]");
      if (incidentButton) {
        event.preventDefault();
        state.selectedIncidentId = incidentButton.dataset.incidentId;
        loadIncidentDetail().then(render).catch(showError);
        return;
      }

      const targetButton = event.target.closest("[data-target-id]");
      if (targetButton) {
        event.preventDefault();
        state.selectedTargetId = targetButton.dataset.targetId;
        loadTargetChecks().then(render).catch(showError);
      }
    });

    window.addEventListener("hashchange", () => {
      syncViewFromHash();
      render();
    });
  }

  function preferredMode() {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }

  function setMode(mode) {
    state.mode = mode === "dark" ? "dark" : "light";
    document.documentElement.dataset.aeMode = state.mode;
    sessionStorage.setItem(MODE_STORAGE, state.mode);
  }

  function syncViewFromHash() {
    const next = window.location.hash.replace("#", "");
    if (VIEWS.includes(next)) {
      state.view = next;
    }
  }

  function setView(view) {
    if (!VIEWS.includes(view)) {
      return;
    }
    state.view = view;
    window.location.hash = view;
    if (view === "incidents" && state.key && !state.data.incidentDetail) {
      loadIncidentDetail().then(render).catch(showError);
    }
  }

  async function refresh() {
    if (!state.key) {
      state.error = "Paste a bearer key to read Canary.";
      render();
      return;
    }

    state.loading = true;
    state.error = "";
    render();

    try {
      const [status, health, incidents] = await Promise.all([
        api("/api/v1/status?window=24h"),
        api("/api/v1/health-status"),
        api("/api/v1/incidents"),
      ]);

      const [timeline, errors, targets, monitors] = await Promise.all([
        optionalApi("/api/v1/timeline?window=7d&limit=100&event_type=incident.opened,incident.updated,incident.resolved"),
        optionalApi("/api/v1/query?group_by=error_class&window=24h"),
        optionalApi("/api/v1/targets"),
        optionalApi("/api/v1/monitors"),
      ]);

      state.data.status = status;
      state.data.health = health;
      state.data.incidents = incidents;
      state.data.timeline = timeline.value;
      state.data.errors = errors.value;
      state.data.targets = targets.value;
      state.data.monitors = monitors.value;
      state.optional.timelineProblem = timeline.problem;
      state.optional.errorsProblem = errors.problem;
      state.optional.adminProblem = [targets.problem, monitors.problem].filter(Boolean).join(" ");

      const incidentIds = incidentChoices().map((incident) => incident.id);
      if (!state.selectedIncidentId || !incidentIds.includes(state.selectedIncidentId)) {
        state.selectedIncidentId = incidentIds[0] || "";
      }
      await loadIncidentDetail();

      const targetsList = healthTargets();
      if (!state.selectedTargetId || !targetsList.some((target) => target.id === state.selectedTargetId)) {
        state.selectedTargetId = targetsList[0]?.id || "";
      }
      await loadTargetChecks();

      state.lastRefresh = new Date();
    } catch (error) {
      state.error = error.message;
    } finally {
      state.loading = false;
      render();
    }
  }

  async function loadIncidentDetail() {
    state.data.incidentDetail = null;
    if (!state.key || !state.selectedIncidentId) {
      return;
    }
    state.data.incidentDetail = await optionalApi(`/api/v1/incidents/${encodeURIComponent(state.selectedIncidentId)}`)
      .then((result) => result.value || { problem: result.problem });
  }

  async function loadTargetChecks() {
    state.data.targetChecks = null;
    if (!state.key || !state.selectedTargetId) {
      return;
    }
    state.data.targetChecks = await optionalApi(`/api/v1/targets/${encodeURIComponent(state.selectedTargetId)}/checks?window=24h`)
      .then((result) => result.value || { problem: result.problem });
  }

  async function api(path) {
    const response = await fetch(path, {
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${state.key}`,
      },
    });
    if (!response.ok) {
      throw new Error(await problemMessage(response));
    }
    return response.json();
  }

  async function optionalApi(path) {
    try {
      return { value: await api(path), problem: "" };
    } catch (error) {
      return { value: null, problem: error.message };
    }
  }

  async function problemMessage(response) {
    const fallback = `${response.status} ${response.statusText}`;
    try {
      const body = await response.json();
      return body.detail || body.title || body.code || fallback;
    } catch (_error) {
      return fallback;
    }
  }

  function render() {
    updateChrome();
    updateNav();

    if (!state.key) {
      root.innerHTML = lockedView();
      return;
    }
    if (state.loading && !state.data.status) {
      root.innerHTML = loadingView();
      return;
    }
    if (state.error) {
      root.innerHTML = problemView(state.error);
      return;
    }

    if (state.view === "incidents") {
      root.innerHTML = renderIncidents();
    } else if (state.view === "checks") {
      root.innerHTML = renderChecks();
    } else if (state.view === "errors") {
      root.innerHTML = renderErrors();
    } else {
      root.innerHTML = renderHealth();
    }
  }

  function updateChrome() {
    connection.textContent = state.key ? "Session key loaded" : "No session key";
    lastRefresh.textContent = state.lastRefresh
      ? `Refreshed ${formatTime(state.lastRefresh.toISOString())}`
      : "Not refreshed";
  }

  function updateNav() {
    document.querySelectorAll("[data-view]").forEach((button) => {
      const active = button.dataset.view === state.view;
      button.toggleAttribute("aria-current", active);
      button.classList.toggle("is-active", active);
    });
  }

  function lockedView() {
    return `
      <section class="view">
        <div class="view-head">
          <div>
            <h1 class="view-title">Canary dashboard</h1>
            <p class="summary">Paste a scoped API key to read this instance.</p>
          </div>
        </div>
        <div class="panel">
          <p class="empty">The shell is public. Canary data is not read until the browser sends your session key to the existing API routes.</p>
        </div>
      </section>
    `;
  }

  function loadingView() {
    const card = `
      <div class="service-row">
        <span class="ae-skeleton">status placeholder</span>
        <span class="ae-skeleton">metric placeholder</span>
      </div>
    `;
    return `
      <section class="view">
        <p class="ae-sr" role="status">Reading Canary — loading current state.</p>
        <div class="view-head">
          <div>
            <h1 class="ae-skeleton view-title">Reading Canary</h1>
            <p class="ae-skeleton summary">Loading current state, one moment.</p>
          </div>
        </div>
        <div class="wall">${card}${card}${card}</div>
      </section>
    `;
  }

  function problemView(message) {
    return `
      <section class="view">
        <h1 class="view-title">Canary rejected the read</h1>
        <div class="problem">${escapeHtml(message)}</div>
      </section>
    `;
  }

  function renderHealth() {
    const rows = serviceRows();
    const status = state.data.status || {};
    const counts = countServiceStates(rows);
    return `
      <section class="view">
        <div class="view-head">
          <div>
            <h1 class="view-title">Production health</h1>
            <p class="summary">${escapeHtml(status.summary || "No status summary returned.")}</p>
          </div>
          <dl class="metric-strip">
            ${metric("Services", rows.length)}
            ${metric("Up", counts.up)}
            ${metric("Degraded", counts.degraded)}
            ${metric("Down", counts.down)}
          </dl>
        </div>
        <div class="wall">
          ${rows.length ? rows.map(renderServiceRow).join("") : empty("No services configured.")}
        </div>
      </section>
    `;
  }

  function renderServiceRow(row) {
    return `
      <article class="service-row status-${row.state}">
        <div>
          <div class="row-main">
            <span class="status-mark">${statusGlyph(row.state)}</span>
            <span class="strong">${escapeHtml(row.service)}</span>
            <span class="tag">${escapeHtml(row.state)}</span>
          </div>
          <div class="meta">${escapeHtml(row.surfaceSummary)}</div>
        </div>
        <div>
          <div>${escapeHtml(row.uptime)}</div>
          <div class="meta">last ${escapeHtml(formatTime(row.lastProbe))}</div>
          <div class="mini-sequence">${escapeHtml(row.sequence)}</div>
        </div>
      </article>
    `;
  }

  function renderIncidents() {
    const incidents = incidentChoices();
    const detail = state.data.incidentDetail;
    return `
      <section class="view">
        <div class="view-head">
          <div>
            <h1 class="view-title">Incidents</h1>
            <p class="summary">${escapeHtml(state.data.incidents?.summary || "No incident summary returned.")}</p>
          </div>
          <dl class="metric-strip">
            ${metric("Open", state.data.incidents?.incidents?.length || 0)}
            ${metric("History", incidentHistory().length)}
            ${metric("Selected", state.selectedIncidentId || "none")}
          </dl>
        </div>
        <div class="incident-layout">
          <aside class="panel">
            <div class="eyebrow">Open and recent</div>
            ${incidents.length ? incidents.map(renderIncidentChoice).join("") : empty("No incidents in the returned window.")}
          </aside>
          <div class="panel">
            ${renderIncidentDetail(detail)}
          </div>
        </div>
      </section>
    `;
  }

  function renderIncidentChoice(incident) {
    const selected = incident.id === state.selectedIncidentId;
    return `
      <button class="select-row ${selected ? "is-selected" : ""}" type="button" data-incident-id="${escapeHtml(incident.id)}">
        <span class="row-main">
          <span class="status-mark ${incident.severity === "high" ? "err" : "warn"}">${incident.severity === "high" ? "x" : "!"}</span>
          <span class="strong">${escapeHtml(incident.title || incident.service || incident.id)}</span>
        </span>
        <span class="meta">${escapeHtml(incident.id)} · ${escapeHtml(incident.state || "event")} · ${escapeHtml(formatTime(incident.opened_at || incident.created_at))}</span>
      </button>
    `;
  }

  function renderIncidentDetail(detail) {
    if (!state.selectedIncidentId) {
      return empty("Select an incident.");
    }
    if (!detail) {
      return empty("Loading incident detail.");
    }
    if (detail.problem) {
      return `<div class="problem">${escapeHtml(detail.problem)}</div>`;
    }
    const incident = detail.incident || {};
    const claim = detail.action_brief?.current_claim || detail.claims?.find((item) => isActiveClaim(item.state));
    return `
      <div>
        <div class="view-head">
          <div>
            <h2 class="view-title">${escapeHtml(incident.title || `${incident.service || "service"} incident`)}</h2>
            <p class="summary">${escapeHtml(detail.summary || "")}</p>
          </div>
          <div class="meta">${escapeHtml(incident.id || "")}</div>
        </div>
        <dl class="detail-grid">
          ${metric("State", incident.state || "unknown")}
          ${metric("Severity", incident.severity || "unknown")}
          ${metric("Opened", formatTime(incident.opened_at))}
          ${metric("Resolution", incident.resolved_at ? formatTime(incident.resolved_at) : "open")}
        </dl>
        <div class="detail-columns">
          <section>
            <div class="eyebrow">Timeline</div>
            ${detail.recent_timeline_events?.length ? `<ol class="ae-trail">${detail.recent_timeline_events.map(renderTimelineEvent).join("")}</ol>` : empty("No recent timeline events.")}
          </section>
          <section>
            <div class="eyebrow">Probe evidence</div>
            ${detail.signals?.length ? detail.signals.map(renderSignal).join("") : empty("No bounded signals returned.")}
          </section>
          <section>
            <div class="eyebrow">Agent activity</div>
            ${claim || detail.annotations?.length ? `<ol class="ae-trail">${renderClaim(claim)}${detail.annotations?.length ? detail.annotations.map(renderActivity).join("") : ""}</ol>` : empty("No writeback annotations yet.")}
          </section>
        </div>
      </div>
    `;
  }

  function renderTimelineEvent(event) {
    return `
      <li class="ae-trail-item">
        <div class="ae-trail-head">
          <span class="ae-trail-time">${escapeHtml(formatTime(event.created_at))}</span>
          <span class="ae-trail-who">${escapeHtml(event.event)}</span>
        </div>
        <div class="ae-trail-body">${escapeHtml(event.summary || "")}</div>
      </li>
    `;
  }

  function renderSignal(signal) {
    const title = signal.summary || signal.error_class || signal.target_name || signal.monitor_name || signal.signal_ref || signal.type;
    const lines = [
      ["type", signal.type],
      ["state", signal.current_state],
      ["count", signal.total_count],
      ["target", signal.target_id],
      ["monitor", signal.monitor_id],
      ["group", signal.group_hash],
      ["annotations", signal.annotation_count],
    ].filter(([_key, value]) => value !== undefined && value !== null && value !== "");
    return `
      <div class="signal-row">
        <div class="strong">${escapeHtml(title || "signal")}</div>
        <div class="metadata">${escapeHtml(lines.map(([key, value]) => `${key}: ${value}`).join("\n"))}</div>
      </div>
    `;
  }

  function renderClaim(claim) {
    if (!claim) {
      return "";
    }
    const lines = [
      ["claim", claim.id],
      ["state", claim.state],
      ["purpose", claim.purpose],
      ["updated", formatTime(claim.updated_at)],
    ];
    return `
      <li class="ae-trail-item is-active">
        <div class="ae-trail-head">
          <span class="ae-trail-time">claim</span>
          <span class="ae-trail-who">${escapeHtml(claim.owner || "agent")}</span>
        </div>
        <div class="ae-trail-body metadata">${escapeHtml(lines.map(([key, value]) => `${key}: ${value || "n/a"}`).join("\n"))}</div>
      </li>
    `;
  }

  function renderActivity(annotation) {
    return `
      <li class="ae-trail-item">
        <div class="ae-trail-head">
          <span class="ae-trail-time">${escapeHtml(formatTime(annotation.created_at))}</span>
          <span class="ae-trail-who">${escapeHtml(annotation.agent || "agent")}</span>
        </div>
        <div class="ae-trail-body">${escapeHtml(annotation.action || "annotated")}</div>
        ${annotation.metadata ? `<div class="metadata">${escapeHtml(JSON.stringify(annotation.metadata, null, 2))}</div>` : ""}
      </li>
    `;
  }

  function renderChecks() {
    const targets = healthTargets();
    const monitors = healthMonitors();
    return `
      <section class="view">
        <div class="view-head">
          <div>
            <h1 class="view-title">Checks and monitors</h1>
            <p class="summary">Current watched surfaces from Canary health-status.</p>
          </div>
          <dl class="metric-strip">
            ${metric("Targets", targets.length)}
            ${metric("Monitors", monitors.length)}
            ${metric("Cadence", state.data.targets || state.data.monitors ? "exact" : "scoped")}
          </dl>
        </div>
        <div class="checks-layout">
          <section class="panel">
            <div class="row-grid table-head">
              <span>Surface</span><span>State</span><span>Cadence</span><span>Last result</span>
            </div>
            ${targets.map(renderTargetRow).join("")}
            ${monitors.map(renderMonitorRow).join("")}
            ${!targets.length && !monitors.length ? empty("No watched surfaces returned.") : ""}
          </section>
          <section class="panel">
            <div class="eyebrow">Recent target checks</div>
            ${renderTargetChecks()}
          </section>
        </div>
      </section>
    `;
  }

  function renderTargetRow(target) {
    const config = targetConfig(target.id);
    return `
      <button class="select-row" type="button" data-target-id="${escapeHtml(target.id)}">
        <span class="row-grid">
          <span><span class="strong">${escapeHtml(target.name)}</span><br><span class="meta">${escapeHtml(target.service)} · ${escapeHtml(target.url)}</span></span>
          <span class="status-${surfaceState(target.state)}"><span class="status-mark">${statusGlyph(surfaceState(target.state))}</span> ${escapeHtml(target.state)}</span>
          <span>${escapeHtml(config?.interval_ms ? duration(config.interval_ms) : inferredCadence(target.recent_checks))}</span>
          <span>${escapeHtml(lastTargetResult(target))}</span>
        </span>
      </button>
    `;
  }

  function renderMonitorRow(monitor) {
    const config = monitorConfig(monitor.id);
    return `
      <div class="data-row row-grid">
        <span><span class="strong">${escapeHtml(monitor.name)}</span><br><span class="meta">${escapeHtml(monitor.service)} · ${escapeHtml(monitor.mode)}</span></span>
        <span class="status-${surfaceState(monitor.state)}"><span class="status-mark">${statusGlyph(surfaceState(monitor.state))}</span> ${escapeHtml(monitor.state)}</span>
        <span>${escapeHtml(duration(config?.expected_every_ms || monitor.expected_every_ms))}</span>
        <span>${escapeHtml(monitor.last_check_in_status || "none")}<br><span class="meta">${escapeHtml(formatTime(monitor.last_check_in_at || monitor.deadline_at))}</span></span>
      </div>
    `;
  }

  function renderTargetChecks() {
    const checks = state.data.targetChecks?.checks || [];
    if (state.data.targetChecks?.problem) {
      return `<div class="problem">${escapeHtml(state.data.targetChecks.problem)}</div>`;
    }
    if (!checks.length) {
      return empty("No recent target checks returned.");
    }
    return checks.map((check) => `
      <div class="data-row">
        <div class="row-main">
          <span class="status-mark ${check.result === "success" ? "ok" : "err"}">${check.result === "success" ? "ok" : "x"}</span>
          <span class="strong">${escapeHtml(check.result || "check")}</span>
          <span class="meta">${escapeHtml(formatTime(check.checked_at))}</span>
        </div>
        <div class="meta">${escapeHtml(check.status_code || "no status")} · ${escapeHtml(check.latency_ms ? `${check.latency_ms}ms` : "no latency")}</div>
      </div>
    `).join("");
  }

  function renderErrors() {
    const errors = state.data.errors;
    const serviceErrors = state.data.status?.error_summary || [];
    return `
      <section class="view">
        <div class="view-head">
          <div>
            <h1 class="view-title">Grouped errors</h1>
            <p class="summary">${escapeHtml(errors?.summary || state.optional.errorsProblem || "No grouped error response returned.")}</p>
          </div>
          <dl class="metric-strip">
            ${metric("Errors", errors?.total_errors ?? errorTotal(serviceErrors))}
            ${metric("Classes", errors?.total_error_classes ?? "scoped")}
            ${metric("Services", serviceErrors.length)}
          </dl>
        </div>
        <div class="errors-layout">
          <section class="panel">
            <div class="row-grid table-head">
              <span>Error class</span><span>Count</span><span>Services</span><span>Window</span>
            </div>
            ${errors?.groups?.length ? errors.groups.map((group) => `
              <div class="data-row row-grid">
                <span class="strong">${escapeHtml(group.error_class)}</span>
                <span>${escapeHtml(group.total_count)}</span>
                <span>${escapeHtml(group.service_count)}</span>
                <span>${escapeHtml(errors.window || "24h")}</span>
              </div>
            `).join("") : empty("No error-class groups returned for this key.")}
          </section>
          <section class="panel">
            <div class="eyebrow">Services with active errors</div>
            ${serviceErrors.length ? serviceErrors.map((item) => `
              <div class="data-row">
                <div class="strong">${escapeHtml(item.service)}</div>
                <div class="meta">${escapeHtml(item.total_count)} errors across ${escapeHtml(item.unique_classes)} classes</div>
              </div>
            `).join("") : empty("No active service errors in the status window.")}
          </section>
        </div>
      </section>
    `;
  }

  function serviceRows() {
    const services = new Map();
    for (const target of healthTargets()) {
      const row = ensureService(services, target.service || target.name);
      row.targets.push(target);
      row.lastProbe = latest(row.lastProbe, target.last_checked_at);
      row.states.push(surfaceState(target.state));
      row.sequences.push(sequence(target.recent_checks));
      row.checks.push(...(target.recent_checks || []));
    }
    for (const monitor of healthMonitors()) {
      const row = ensureService(services, monitor.service || monitor.name);
      row.monitors.push(monitor);
      row.lastProbe = latest(row.lastProbe, monitor.last_check_in_at || monitor.deadline_at);
      row.states.push(surfaceState(monitor.state));
      row.sequences.push(monitor.last_check_in_status || "none");
    }
    for (const item of state.data.status?.error_summary || []) {
      const row = ensureService(services, item.service);
      row.errorCount += Number(item.total_count || 0);
      row.errorClasses += Number(item.unique_classes || 0);
    }
    return [...services.values()].map((row) => {
      row.state = row.states.includes("down")
        ? "down"
        : row.states.includes("degraded") || row.errorCount > 0
          ? "degraded"
          : row.states.length
            ? "up"
            : "unknown";
      const totalChecks = row.checks.length;
      const successes = row.checks.filter((check) => check.result === "success").length;
      row.uptime = totalChecks ? `${Math.round((successes / totalChecks) * 1000) / 10}%` : "n/a";
      row.sequence = row.sequences.filter(Boolean).join("  ") || "no recent checks";
      row.surfaceSummary = `${row.targets.length} targets · ${row.monitors.length} monitors · ${row.errorCount} errors`;
      return row;
    }).sort((a, b) => stateWeight(a.state) - stateWeight(b.state) || a.service.localeCompare(b.service));
  }

  function ensureService(map, service) {
    const key = service || "unknown";
    if (!map.has(key)) {
      map.set(key, {
        service: key,
        targets: [],
        monitors: [],
        states: [],
        checks: [],
        sequences: [],
        errorCount: 0,
        errorClasses: 0,
        lastProbe: "",
      });
    }
    return map.get(key);
  }

  function incidentChoices() {
    const active = (state.data.incidents?.incidents || []).map((incident) => ({ ...incident, source: "open" }));
    const known = new Set(active.map((incident) => incident.id));
    const history = incidentHistory().filter((incident) => !known.has(incident.id));
    return active.concat(history);
  }

  function incidentHistory() {
    const rows = [];
    const seen = new Set();
    for (const event of state.data.timeline?.events || []) {
      if (event.entity_type !== "incident" || !event.entity_ref || seen.has(event.entity_ref)) {
        continue;
      }
      seen.add(event.entity_ref);
      rows.push({
        id: event.entity_ref,
        service: event.service,
        state: event.event.replace("incident.", ""),
        severity: event.severity || "medium",
        title: event.summary,
        opened_at: event.created_at,
        created_at: event.created_at,
        source: "history",
      });
    }
    return rows;
  }

  function healthTargets() {
    return state.data.health?.targets || [];
  }

  function healthMonitors() {
    return state.data.health?.monitors || [];
  }

  function targetConfig(id) {
    return (state.data.targets?.targets || []).find((target) => target.id === id);
  }

  function monitorConfig(id) {
    return (state.data.monitors?.monitors || []).find((monitor) => monitor.id === id);
  }

  function countServiceStates(rows) {
    return rows.reduce((acc, row) => {
      acc[row.state] = (acc[row.state] || 0) + 1;
      return acc;
    }, { up: 0, degraded: 0, down: 0, unknown: 0 });
  }

  function stateWeight(value) {
    return { down: 0, degraded: 1, unknown: 2, up: 3 }[value] ?? 4;
  }

  function surfaceState(value) {
    if (value === "up" || value === "healthy") {
      return "up";
    }
    if (value === "down" || value === "unhealthy") {
      return "down";
    }
    if (value === "degraded" || value === "warning") {
      return "degraded";
    }
    return "unknown";
  }

  function statusGlyph(value) {
    if (value === "up") {
      return "ok";
    }
    if (value === "down") {
      return "x";
    }
    if (value === "degraded") {
      return "!";
    }
    return "-";
  }

  function sequence(checks) {
    if (!checks || !checks.length) {
      return "";
    }
    return checks.slice(0, 8).map((check) => check.result === "success" ? "ok" : "x").join(" ");
  }

  function lastTargetResult(target) {
    const latestCheck = target.recent_checks?.[0];
    if (!latestCheck) {
      return target.last_checked_at ? `checked ${formatTime(target.last_checked_at)}` : "none";
    }
    return `${latestCheck.result || "check"} ${formatTime(latestCheck.checked_at)}`;
  }

  function inferredCadence(checks) {
    if (!checks || checks.length < 2) {
      return "scoped";
    }
    const first = Date.parse(checks[0].checked_at);
    const second = Date.parse(checks[1].checked_at);
    if (!Number.isFinite(first) || !Number.isFinite(second)) {
      return "scoped";
    }
    return duration(Math.abs(first - second));
  }

  function latest(left, right) {
    if (!right) {
      return left;
    }
    if (!left) {
      return right;
    }
    return Date.parse(right) > Date.parse(left) ? right : left;
  }

  function duration(ms) {
    const value = Number(ms || 0);
    if (!value) {
      return "n/a";
    }
    const seconds = Math.round(value / 1000);
    if (seconds < 60) {
      return `${seconds}s`;
    }
    const minutes = Math.round(seconds / 60);
    if (minutes < 60) {
      return `${minutes}m`;
    }
    return `${Math.round(minutes / 60)}h`;
  }

  function formatTime(value) {
    if (!value) {
      return "n/a";
    }
    const date = value instanceof Date ? value : new Date(value);
    if (Number.isNaN(date.getTime())) {
      return String(value);
    }
    return new Intl.DateTimeFormat(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hour12: false,
    }).format(date);
  }

  function errorTotal(items) {
    return items.reduce((total, item) => total + Number(item.total_count || 0), 0);
  }

  function isActiveClaim(value) {
    return ["claimed", "investigating", "fix_proposed", "verified"].includes(value);
  }

  function metric(label, value) {
    return `<div class="metric"><dt>${escapeHtml(label)}</dt><dd>${escapeHtml(value)}</dd></div>`;
  }

  function empty(message) {
    return `<div class="empty">${escapeHtml(message)}</div>`;
  }

  function showError(error) {
    state.error = error.message;
    state.loading = false;
    render();
  }

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#39;");
  }

  init();
})();
