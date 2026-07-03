(() => {
  const KEY_STORAGE = "canary.dashboard.sessionKey";
  const MODE_STORAGE = "canary.dashboard.mode";
  const FIGURES = ["all", "up", "down", "incidents", "errors"];
  const APP_ICONS = [
    ["bitterblossom", "i-flower"],
    ["powder", "i-kanban"],
    ["crucible", "i-flask-conical"],
    ["cerberus", "i-eye"],
    ["landmark", "i-milestone"],
    ["harness-kit", "i-toolbox"],
    ["weave", "i-layers"],
    ["canary", "i-bird"],
  ];

  const state = {
    figure: "all",
    view: "all",
    key: storedSessionKey(),
    mode: sessionStorage.getItem(MODE_STORAGE) || "",
    loading: false,
    error: "",
    lastRefresh: null,
    selectedSheetId: "",
    data: {
      status: null,
      health: null,
      incidents: null,
      timeline: null,
      errors: null,
      targets: null,
      monitors: null,
      incidentDetails: new Map(),
    },
    optional: {
      errorsProblem: "",
      timelineProblem: "",
      adminProblem: "",
    },
  };

  const root = document.getElementById("view-root");
  const authChrome = document.getElementById("auth-chrome");
  const lastRefresh = document.getElementById("last-refresh");
  const sheet = document.getElementById("sheet");
  const scrim = document.getElementById("scrim");

  function init() {
    setMode(state.mode || preferredMode());
    bindEvents();
    render();
    if (state.key) {
      refresh();
    }
  }

  function bindEvents() {
    authChrome.addEventListener("submit", (event) => {
      if (!event.target.closest("#session-form")) {
        return;
      }
      event.preventDefault();
      const keyInput = document.getElementById("api-key");
      state.key = keyInput.value.trim();
      if (state.key) {
        localStorage.setItem(KEY_STORAGE, state.key);
        sessionStorage.removeItem(KEY_STORAGE);
      }
      refresh();
    });

    authChrome.addEventListener("click", (event) => {
      if (!event.target.closest("[data-clear-key]")) {
        return;
      }
      state.key = "";
      localStorage.removeItem(KEY_STORAGE);
      sessionStorage.removeItem(KEY_STORAGE);
      state.error = "";
      state.data = {
        status: null,
        health: null,
        incidents: null,
        timeline: null,
        errors: null,
        targets: null,
        monitors: null,
        incidentDetails: new Map(),
      };
      closeSheet();
      render();
    });

    document.getElementById("refresh").addEventListener("click", () => refresh());
    document.getElementById("mode-toggle").addEventListener("click", () => {
      setMode(document.documentElement.dataset.aeMode === "dark" ? "light" : "dark");
    });

    document.addEventListener("click", (event) => {
      const fig = event.target.closest("[data-fig]");
      if (fig) {
        event.preventDefault();
        setFigure(fig.dataset.fig);
        return;
      }

      if (event.target.closest("[data-clear]")) {
        event.preventDefault();
        setFigure("all");
        return;
      }

      const view = event.target.closest("[data-view]");
      if (view) {
        event.preventDefault();
        state.view = view.dataset.view;
        closeSheet();
        render();
        return;
      }

      const incident = event.target.closest("[data-incident-id]");
      if (incident) {
        event.preventDefault();
        openIncidentSheet(incident.dataset.incidentId);
        return;
      }

      if (event.target.closest("[data-close]") || event.target === scrim) {
        closeSheet();
      }
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

  function storedSessionKey() {
    const durable = localStorage.getItem(KEY_STORAGE) || "";
    if (durable) {
      return durable;
    }
    const legacy = sessionStorage.getItem(KEY_STORAGE) || "";
    if (legacy) {
      localStorage.setItem(KEY_STORAGE, legacy);
      sessionStorage.removeItem(KEY_STORAGE);
    }
    return legacy;
  }

  function setFigure(figure) {
    if (!FIGURES.includes(figure)) {
      return;
    }
    state.figure = figure;
    if (figure === "incidents" || figure === "errors") {
      state.view = "all";
    } else if (state.view !== "all" && !visibleApps().some((app) => app.key === state.view)) {
      state.view = "all";
    }
    closeSheet();
    render();
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
        optionalApi("/api/v1/timeline?window=7d&limit=100"),
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
      state.data.incidentDetails = new Map();
      state.optional.timelineProblem = timeline.problem;
      state.optional.errorsProblem = errors.problem;
      state.optional.adminProblem = [targets.problem, monitors.problem].filter(Boolean).join(" ");

      const ids = incidentFeed().map((incident) => incident.id);
      await Promise.all(ids.slice(0, 8).map(loadIncidentDetail));
      state.lastRefresh = new Date();
    } catch (error) {
      state.error = error.message;
    } finally {
      state.loading = false;
      render();
    }
  }

  async function openIncidentSheet(id) {
    state.selectedSheetId = id;
    renderSheet();
    if (!state.data.incidentDetails.has(id)) {
      await loadIncidentDetail(id);
    }
    renderSheet();
  }

  async function loadIncidentDetail(id) {
    if (!state.key || !id || state.data.incidentDetails.has(id)) {
      return;
    }
    const result = await optionalApi(`/api/v1/incidents/${encodeURIComponent(id)}`);
    state.data.incidentDetails.set(id, result.value || { problem: result.problem });
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
    closeStaleSheet();
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
    root.innerHTML = dashboardView();
    renderSheet();
  }

  function updateChrome() {
    renderAuthChrome();
    lastRefresh.textContent = state.lastRefresh
      ? `Refreshed ${formatTime(state.lastRefresh.toISOString())}`
      : "Not refreshed";
  }

  function renderAuthChrome() {
    authChrome.innerHTML = state.key ? "" : `
      <form class="session-form" id="session-form">
        <label for="api-key">Bearer key</label>
        <input id="api-key" name="api-key" type="password" autocomplete="off" spellcheck="false" placeholder="paste bearer key">
        <button type="submit">Sign in</button>
      </form>
    `;
  }

  function lockedView() {
    return `
      <section class="view">
        ${figuresMarkup({ apps: 0, up: 0, down: 0, incidents: 0, errors: 0 })}
        <div class="split">
          <nav class="master" aria-label="Destinations">
            <p class="ae-h">SESSION</p>
            <div class="empty">The shell is public. Paste a scoped bearer key to read Canary.</div>
          </nav>
          <section class="detail">
            <div class="ae-view">
              <div class="ae-group">
                <p><span class="ae-strong">Canary dashboard</span></p>
                <p class="ae-chrome">Canary data is not read until this browser sends your session key to the existing API routes.</p>
              </div>
            </div>
          </section>
        </div>
      </section>
    `;
  }

  function loadingView() {
    return `
      <section class="view">
        <p class="ae-sr" role="status">Reading Canary.</p>
        ${figuresMarkup({ apps: 0, up: 0, down: 0, incidents: 0, errors: 0 })}
        <div class="split">
          <nav class="master" aria-label="Destinations">
            <p class="ae-h">READING</p>
            <div class="empty">Loading current state.</div>
          </nav>
          <section class="detail">
            <div class="ae-view"><div class="ae-group"><p class="ae-skeleton">Reading Canary</p><p class="ae-skeleton">Loading current status, health, incidents, and annotations.</p></div></div>
          </section>
        </div>
      </section>
    `;
  }

  function problemView(message) {
    return `
      <section class="view">
        ${figuresMarkup(metrics())}
        <div class="split">
          <nav class="master" aria-label="Destinations">
            <p class="ae-h">ERROR</p>
          </nav>
          <section class="detail">
            <h1 class="ae-strong">Canary rejected the read</h1>
            <div class="problem">${escapeHtml(message)}</div>
          </section>
        </div>
      </section>
    `;
  }

  function dashboardView() {
    return `
      <section class="view">
        ${figuresMarkup(metrics())}
        <div class="split">
          ${masterMarkup()}
          <section class="detail" aria-live="polite">${detailMarkup()}</section>
        </div>
      </section>
    `;
  }

  function figuresMarkup(values) {
    const rows = [
      ["all", "APPLICATIONS", "i-grid", "", values.apps],
      ["up", "UP", "i-check", "ae-ok", values.up],
      ["down", "DOWN", "i-x", "ae-err", values.down],
      ["incidents", "OPEN INCIDENTS", "i-alert", "ae-warn", values.incidents],
      ["errors", "ERRORS 24H", "i-zap", "ae-err", values.errors],
    ];
    return `<div class="figures" role="group" aria-label="Fleet figures">${rows.map(([key, label, iconId, cls, value]) => `
      <button class="figure" type="button" data-fig="${key}" aria-pressed="${state.figure === key}">
        <span class="figure-label">${label}</span>
        <span class="figure-val">${iconSvg(iconId, cls)}<span class="ae-num">${escapeHtml(value)}</span></span>
      </button>`).join("")}</div>`;
  }

  function masterMarkup() {
    const apps = visibleApps();
    const allCount = allApps().length;
    const filterNote = state.figure === "all"
      ? ""
      : `<span class="sub">filtered by the ${state.figure === "incidents" ? "open-incidents" : state.figure} square · <a href="#" data-clear>clear</a></span>`;
    return `
      <nav class="master" aria-label="Destinations">
        <p class="ae-h">INCIDENTS</p>
        <button class="dest" type="button" aria-selected="${state.view === "all"}" data-view="all">
          ${iconSvg("i-inbox", "logo")}
          <span class="ae-item">All incidents</span>
          <span class="fig">${glyph("warn")}<span>${openIncidentCount()} open</span></span>
        </button>
        <p class="ae-h">APPLICATIONS${state.figure !== "all" ? ` · ${apps.length} OF ${allCount}` : ""}</p>
        ${filterNote}
        ${apps.map((app) => `
          <button class="dest" type="button" aria-selected="${state.view === app.key}" data-view="${escapeHtml(app.key)}">
            ${appIcon(app.name, "logo")}
            <span class="ae-item">${escapeHtml(app.name)}</span>
            <span class="fig">${glyph(app.glyph)}<span>${escapeHtml(app.uptime)}</span></span>
            ${app.incidents.length || app.errorCount ? `<span class="sub">${streamLabel(app.incidents.length)} · ${escapeHtml(app.meta)}</span>` : ""}
          </button>
        `).join("")}
      </nav>
    `;
  }

  function detailMarkup() {
    if (state.view === "all") {
      return allIncidentsView();
    }
    const app = allApps().find((item) => item.key === state.view);
    return app ? appView(app) : allIncidentsView();
  }

  const incHead = `
    <div class="inc-head" aria-hidden="true">
      <span></span><span>TIME</span><span>TYPE</span><span>APPLICATION</span><span>WHAT HAPPENED</span><span>STATE</span>
    </div>`;

  function allIncidentsView() {
    const feed = incidentFeed();
    return `
      <div class="ae-view">
        <div class="ae-group">
          <p>${iconSvg("i-inbox")} <span class="ae-strong">All incidents</span></p>
          <p class="ae-chrome">${escapeHtml(state.data.incidents?.summary || "Every error group, failed check, and outage across the fleet resolves into one incident stream.")}</p>
        </div>
        <div class="ae-group">
          ${feed.length ? incHead + feed.map(incidentRow).join("") : empty("No incidents in the returned window.")}
        </div>
      </div>
    `;
  }

  function appView(app) {
    const rows = app.incidents.length
      ? incHead + app.incidents.map(incidentRow).join("")
      : `<div class="ae-empty"><p>${glyph("ok")} <span class="ae-item">Calm.</span></p><p class="ae-dim">No open incidents. Errors, failed checks, and downtime surface here.</p></div>`;
    const trail = app.probes.length
      ? `<p class="ae-chrome">last five probes, oldest first · ${trail5(app.probes, `Last five probes: ${app.probes.join(", ")}`)}</p>`
      : `<p class="ae-chrome">no probe trail - this app may report by monitor/check-in.</p>`;
    return `
      <div class="ae-view">
        <div class="ae-group">
          <p class="apphead">${appIcon(app.name)} <span class="ae-strong">${escapeHtml(app.name)}</span> ${glyph(app.glyph)}</p>
          <p class="ae-chrome ae-num">uptime ${escapeHtml(app.uptime)} · last probe ${escapeHtml(formatTime(app.lastProbe))} · ${escapeHtml(app.meta)}</p>
          ${trail}
        </div>
        <div class="ae-group">
          <p class="ae-h">INCIDENT STREAM</p>
          ${rows}
        </div>
      </div>
    `;
  }

  function incidentRow(incident) {
    const severityGlyph = incident.escalated_at ? "err" : incident.severity === "high" ? "err" : "warn";
    return `
      <button class="inc ${incident.escalated_at ? "is-escalated" : ""}" type="button" data-incident-id="${escapeHtml(incident.id)}">
        <span class="glyph">${glyph(severityGlyph)}</span>
        <span class="t">${escapeHtml(formatTime(incident.opened_at || incident.created_at))}</span>
        <span class="type"><span class="ae-tag">${escapeHtml(incidentType(incident))}</span></span>
        <span class="app">${appIcon(incident.service, "")}${escapeHtml(incident.service || "unknown")}</span>
        <span class="ae-item">${escapeHtml(incident.title || incident.summary || incident.id)}</span>
        <span class="state"><span class="ae-tag">${escapeHtml(incident.escalated_at ? "ESCALATED" : incident.state || "open")}</span></span>
      </button>
    `;
  }

  function renderSheet() {
    if (!state.selectedSheetId) {
      closeSheet();
      return;
    }
    const incident = incidentFeed().find((item) => item.id === state.selectedSheetId);
    const detail = state.data.incidentDetails.get(state.selectedSheetId);
    sheet.innerHTML = sheetMarkup(incident, detail);
    sheet.classList.add("is-on");
    scrim.classList.add("is-on");
  }

  function sheetMarkup(incident, detail) {
    if (!incident) {
      return `${sheetClose()}<div class="problem">Incident no longer appears in the current feed.</div>`;
    }
    if (detail?.problem) {
      return `${sheetClose()}<div class="problem">${escapeHtml(detail.problem)}</div>`;
    }
    const richIncident = { ...incident, ...(detail?.incident || {}) };
    const signals = detail?.signals || incident.signals || [];
    const annotations = detail?.annotations || [];
    const events = incidentTimelineEvents(richIncident.id || incident.id, detail);
    const claim = detail?.action_brief?.current_claim || detail?.claims?.find((item) => isActiveClaim(item.state));
    const escalation = richIncident.escalated_at ? `escalated ${formatTime(richIncident.escalated_at)}` : "escalation none";
    const resolution = richIncident.resolved_at ? `resolved ${formatTime(richIncident.resolved_at)}` : "resolution pending";
    return `${sheetClose()}
      <div class="ae-group sheet-state">
        <p>${glyph(richIncident.escalated_at || incident.escalated_at ? "err" : "warn")} <span class="ae-strong ae-num">${escapeHtml(richIncident.id || incident.id)}</span></p>
        <p class="ae-chrome">
          <span class="ae-tag">${escapeHtml(incidentType(richIncident))}</span>
          <span class="ae-tag">severity ${escapeHtml(richIncident.severity || "medium")}</span>
          <span class="ae-tag">${escapeHtml(richIncident.state || "open")}</span>
          <span class="ae-tag">${escapeHtml(escalation)}</span>
          <span class="ae-num">opened ${escapeHtml(formatTime(richIncident.opened_at))} · ${signals.length || richIncident.signal_count || 0} signals</span>
        </p>
      </div>
      <div class="ae-group">
        <div class="ae-plate">
          <p class="ae-plate-cap">WHAT IS GOING ON</p>
          <div class="sheet-glance">
            <div><span>what</span><strong>${escapeHtml(richIncident.title || incident.title || "incident")}</strong></div>
            <div><span>where</span><strong>${escapeHtml(richIncident.service || incident.service || "unknown")}</strong></div>
            <div><span>since</span><strong>${escapeHtml(formatTime(richIncident.opened_at || incident.opened_at))}</strong></div>
            <div><span>state</span><strong>${escapeHtml(richIncident.escalated_at ? "escalated" : richIncident.state || "open")}</strong></div>
            <div><span>assignee</span><strong>${escapeHtml(claim?.owner || "unassigned")}</strong></div>
            <div><span>resolution</span><strong>${escapeHtml(resolution)}</strong></div>
          </div>
          ${signals.length ? signals.map(signalLine).join("") : `<div class="evi">${glyph("warn")}<span>No bounded signals returned for this incident.</span></div>`}
          <p class="ae-plate-note">${escapeHtml(detail?.summary || richIncident.title || incident.title || "")}</p>
        </div>
      </div>
      <div class="ae-group">
        <p class="ae-h">EVIDENCE LINKS</p>
        ${evidenceLinks(richIncident, signals).length ? evidenceLinks(richIncident, signals).map(renderEvidenceLink).join("") : `<p class="ae-dim">No deep evidence links returned for this incident.</p>`}
      </div>
      <div class="ae-group">
        <p class="ae-h">ASSIGNEE + WORK LOG</p>
        ${claim ? `<p class="ae-chrome">assigned to ${escapeHtml(claim.owner || "agent")} · ${escapeHtml(claim.purpose || claim.state || "claimed")}</p>` : `<p class="ae-chrome">unassigned · no active claim</p>`}
        ${claim || annotations.length || events.length ? `<ol class="ae-trail">${renderClaim(claim)}${annotations.map(renderAnnotation).join("")}${events.map(renderTimelineEvent).join("")}</ol>` : `<div class="ae-empty"><p>No agent writeback yet.</p><p class="ae-dim">Annotations, claims, and incident timeline events from the real API render here.</p></div>`}
      </div>
      <div class="ae-group">
        <p class="ae-h">RESOLUTION + ESCALATION</p>
        <p class="ae-dim">Proof of resolution: ${escapeHtml(resolution)}.</p>
        <p class="ae-chrome">escalation state: ${escapeHtml(escalation)}</p>
      </div>`;
  }

  function incidentTimelineEvents(id, detail) {
    const localEvents = detail?.recent_timeline_events || [];
    const globalEvents = (state.data.timeline?.events || []).filter((event) => event.entity_type === "incident" && event.entity_ref === id);
    const seen = new Set();
    return localEvents.concat(globalEvents).filter((event) => {
      const key = `${event.event}:${event.created_at}:${event.summary}`;
      if (seen.has(key)) {
        return false;
      }
      seen.add(key);
      return true;
    }).sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  }

  function evidenceLinks(incident, signals) {
    const links = [{ label: "incident detail", href: `/api/v1/incidents/${encodeURIComponent(incident.id)}` }];
    for (const signal of signals) {
      if ((signal.signal_type === "error_group" || signal.type === "error_group") && signal.signal_ref) {
        links.push({ label: signal.summary || signal.signal_ref || "error group", href: `/api/v1/errors/${encodeURIComponent(signal.signal_ref)}` });
      }
    }
    return links;
  }

  function renderEvidenceLink(link) {
    return `<a class="evi-link" href="${escapeHtml(link.href)}" target="_blank" rel="noreferrer">${iconSvg("i-eye")}<span>${escapeHtml(link.label)}</span><span class="ae-chrome">${escapeHtml(link.href)}</span></a>`;
  }

  function signalLine(signal) {
    const text = signal.summary || signal.error_class || signal.target_name || signal.monitor_name || signal.signal_ref || signal.type || signal.signal_type || "signal";
    const right = [
      signal.total_count ? `count ${signal.total_count}` : "",
      signal.consecutive_failures ? `${signal.consecutive_failures} consecutive failures` : "",
      signal.annotation_count != null ? `${signal.annotation_count} annotations` : "",
    ].filter(Boolean).join(" · ");
    return `<div class="evi">${glyph(signal.type === "error_group" || signal.signal_type === "error_group" ? "err" : "warn")}<span>${escapeHtml(text)}</span><span class="ae-chrome ae-num" style="margin-left:auto">${escapeHtml(right || formatTime(signal.attached_at))}</span></div>`;
  }

  function renderClaim(claim) {
    if (!claim) {
      return "";
    }
    return `
      <li class="ae-trail-item is-active">
        <div class="ae-trail-head">
          <span class="ae-trail-time">${escapeHtml(formatTime(claim.updated_at))}</span>
          <span class="ae-trail-who">${escapeHtml(claim.owner || "agent")}</span>
        </div>
        <div class="ae-trail-body">${escapeHtml(claim.purpose || claim.state || "claimed incident")}</div>
      </li>`;
  }

  function renderAnnotation(annotation) {
    return `
      <li class="ae-trail-item">
        <div class="ae-trail-head">
          <span class="ae-trail-time">${escapeHtml(formatTime(annotation.created_at))}</span>
          <span class="ae-trail-who">${escapeHtml(annotation.agent || "agent")}</span>
        </div>
        <div class="ae-trail-body">${escapeHtml(annotation.action || "annotated")}</div>
        ${annotation.metadata ? `<div class="metadata">${escapeHtml(JSON.stringify(annotation.metadata, null, 2))}</div>` : ""}
      </li>`;
  }

  function renderTimelineEvent(event) {
    return `
      <li class="ae-trail-item">
        <div class="ae-trail-head">
          <span class="ae-trail-time">${escapeHtml(formatTime(event.created_at))}</span>
          <span class="ae-trail-who">${escapeHtml(event.event || "timeline")}</span>
        </div>
        <div class="ae-trail-body">${escapeHtml(event.summary || "")}</div>
      </li>`;
  }

  function sheetClose() {
    return `<button class="sheet-x" type="button" data-close aria-label="Close ticket">${iconSvg("i-x")}</button>`;
  }

  function closeSheet() {
    state.selectedSheetId = "";
    sheet.classList.remove("is-on");
    scrim.classList.remove("is-on");
    sheet.innerHTML = "";
  }

  function closeStaleSheet() {
    if (state.selectedSheetId && !incidentFeed().some((item) => item.id === state.selectedSheetId)) {
      closeSheet();
    }
  }

  function metrics() {
    const apps = allApps();
    return {
      apps: apps.length,
      up: apps.filter((app) => app.state === "up").length,
      down: apps.filter((app) => app.state === "down" || app.state === "degraded").length,
      incidents: openIncidentCount(),
      errors: Number(state.data.errors?.total_errors ?? errorTotal(state.data.status?.error_summary || [])),
    };
  }

  function visibleApps() {
    const apps = allApps();
    if (state.figure === "up") {
      return apps.filter((app) => app.state === "up");
    }
    if (state.figure === "down") {
      return apps.filter((app) => app.state !== "up");
    }
    if (state.figure === "incidents") {
      return apps.filter((app) => app.incidents.length);
    }
    if (state.figure === "errors") {
      return apps.filter((app) => app.errorCount > 0);
    }
    return apps;
  }

  function allApps() {
    const services = new Map();
    for (const target of healthTargets()) {
      const row = ensureService(services, target.service || target.name);
      row.targets.push(target);
      row.lastProbe = latest(row.lastProbe, target.last_checked_at);
      row.states.push(surfaceState(target.state));
      row.probes.push(...probeGlyphs(target.recent_checks));
    }
    for (const monitor of healthMonitors()) {
      const row = ensureService(services, monitor.service || monitor.name);
      row.monitors.push(monitor);
      row.lastProbe = latest(row.lastProbe, monitor.last_check_in_at || monitor.deadline_at);
      row.states.push(surfaceState(monitor.state));
    }
    for (const item of state.data.status?.error_summary || []) {
      const row = ensureService(services, item.service);
      row.errorCount += Number(item.total_count || 0);
      row.errorClasses += Number(item.unique_classes || 0);
    }
    for (const incident of incidentFeed()) {
      const row = ensureService(services, incident.service || "unknown");
      row.incidents.push(incident);
    }
    return [...services.values()].map(finalizeService).sort((a, b) => stateWeight(a.state) - stateWeight(b.state) || a.name.localeCompare(b.name));
  }

  function ensureService(map, service) {
    const key = String(service || "unknown");
    if (!map.has(key)) {
      map.set(key, {
        key,
        name: key,
        targets: [],
        monitors: [],
        states: [],
        probes: [],
        incidents: [],
        errorCount: 0,
        errorClasses: 0,
        lastProbe: "",
      });
    }
    return map.get(key);
  }

  function finalizeService(row) {
    row.state = row.states.includes("down")
      ? "down"
      : row.states.includes("degraded") || row.errorCount > 0 || row.incidents.some((incident) => incident.severity === "high")
        ? "degraded"
        : row.states.length
          ? "up"
          : "unknown";
    const targetChecks = row.targets.flatMap((target) => target.recent_checks || []);
    const successes = targetChecks.filter((check) => check.result === "success").length;
    row.uptime = targetChecks.length ? `${Math.round((successes / targetChecks.length) * 1000) / 10}%` : "n/a";
    row.glyph = row.state === "up" ? "ok" : row.state === "down" ? "err" : "warn";
    row.probes = row.probes.slice(-5);
    row.meta = `${row.targets.length} targets · ${row.monitors.length} monitors · ${row.errorCount} errors 24h`;
    return row;
  }

  function incidentFeed() {
    const escalations = escalationMap();
    return activeIncidentFeed().map((incident) => {
      const detail = state.data.incidentDetails.get(incident.id);
      return { ...incident, escalated_at: incident.escalated_at || detail?.incident?.escalated_at || detail?.escalated_at || escalations.get(incident.id) || null };
    });
  }

  function activeIncidentFeed() {
    return (state.data.incidents?.incidents || []).map((incident) => ({ ...incident, source: "open" }));
  }

  function escalationMap() {
    const map = new Map();
    const events = [...(state.data.timeline?.events || [])].sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at));
    for (const event of events) {
      if (event.entity_type !== "incident" || !event.entity_ref) {
        continue;
      }
      if (event.event === "incident.escalated") {
        map.set(event.entity_ref, event.created_at);
      } else if (event.event === "incident.deescalated" || event.event === "incident.resolved") {
        map.delete(event.entity_ref);
      }
    }
    return map;
  }

  function openIncidentCount() {
    return state.data.incidents?.incidents?.length || 0;
  }

  function healthTargets() {
    return state.data.health?.targets || [];
  }

  function healthMonitors() {
    return state.data.health?.monitors || [];
  }

  function probeGlyphs(checks) {
    return (checks || []).slice(0, 5).reverse().map((check) => check.result === "success" ? "ok" : "err");
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

  function stateWeight(value) {
    return { down: 0, degraded: 1, unknown: 2, up: 3 }[value] ?? 4;
  }

  function incidentType(incident) {
    if (incident.signals?.some((signal) => signal.signal_type === "error_group" || signal.type === "error_group")) {
      return "error";
    }
    if (incident.signals?.some((signal) => signal.signal_type === "health_transition" || signal.type === "health_transition")) {
      return "health";
    }
    return incident.source === "history" ? "event" : "incident";
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

  function errorTotal(items) {
    return items.reduce((total, item) => total + Number(item.total_count || 0), 0);
  }

  function incidentLabel(count) {
    if (!count) {
      return "0 open incidents";
    }
    return `${count} open incident${count === 1 ? "" : "s"}`;
  }

  function streamLabel(count) {
    if (!count) {
      return "0 incidents";
    }
    return `${count} incident${count === 1 ? "" : "s"}`;
  }

  function isActiveClaim(value) {
    return ["claimed", "investigating", "fix_proposed", "verified"].includes(value);
  }

  function trail5(probes, label) {
    return `<span class="trail5" role="img" aria-label="${escapeHtml(label)}">${probes.map(glyph).join("")}</span>`;
  }

  function glyph(kind) {
    if (kind === "ok") {
      return iconSvg("i-check", "ae-ok");
    }
    if (kind === "err") {
      return iconSvg("i-x", "ae-err");
    }
    return iconSvg("i-alert", "ae-warn");
  }

  function appIcon(name, cls = "") {
    const normalized = String(name || "").toLowerCase();
    const icon = APP_ICONS.find(([needle]) => normalized.includes(needle))?.[1] || "i-dot";
    return iconSvg(icon, cls);
  }

  function iconSvg(id, cls = "") {
    return `<svg class="ae-icon ${escapeHtml(cls)}" aria-hidden="true"><use href="#${escapeHtml(id)}"></use></svg>`;
  }

  function empty(message) {
    return `<div class="empty">${escapeHtml(message)}</div>`;
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
