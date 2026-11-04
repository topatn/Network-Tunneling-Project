/**
 * Game Tunnel Tooling (GTT) - Desktop Shell Controller (Framed-Sheet Direction)
 * Implements S1 Welcome, S2 Sign-In Handoff, S4 Choose Game (FR-002),
 * S5 Local Server Configuration (FR-003, GAME-009, GAME-008), and S7 Tunnel Dashboard (FR-010, UX-002, UX-006).
 * Governed by: DESIGN.md, user-flows.md, requirements.md, contracts.md §13, references/app-screens.md
 */

// Shipped game profiles matching profiles/*.json (FR-002, GAME-006, GAME-008)
const SHIPPED_PROFILES = [
  {
    id: "minecraft-java",
    name: "Minecraft: Java Edition",
    supportLevel: "verified", // "verified" | "experimental" (GAME-006)
    tagline: "Standard Java multiplayer server (TCP 25565 + optional Query/RCON)",
    engineFamily: "Netty TCP Frame Stream",
    mappings: [
      {
        key: "game",
        protocol: "tcp",
        defaultPort: 25565,
        optional: false,
        description: "Primary multiplayer game connection",
      },
      {
        key: "query",
        protocol: "udp",
        defaultPort: 25565,
        optional: true,
        riskWarning: "Query exposes server status and player counts publicly. Keep disabled unless required.",
      },
      {
        key: "rcon",
        protocol: "tcp",
        defaultPort: 25575,
        optional: true,
        riskWarning: "RCON allows remote administrative console execution. Keep disabled unless required.",
      },
    ],
  },
  {
    id: "minecraft-bedrock",
    name: "Minecraft: Bedrock Edition",
    supportLevel: "experimental",
    tagline: "Cross-platform RakNet UDP dedicated server",
    engineFamily: "RakNet UDP Datagram",
    mappings: [
      {
        key: "game",
        protocol: "udp",
        defaultPort: 19132,
        optional: false,
        description: "Primary Bedrock IPv4 game port",
      },
      {
        key: "game-ipv6",
        protocol: "udp",
        defaultPort: 19133,
        optional: true,
        riskWarning: "Secondary IPv6 listener port for Bedrock.",
      },
    ],
  },
  {
    id: "terraria",
    name: "Terraria",
    supportLevel: "experimental",
    tagline: "Dedicated world server over TCP 7777",
    engineFamily: "XNA/FNA Length-Prefixed TCP",
    mappings: [
      {
        key: "game",
        protocol: "tcp",
        defaultPort: 7777,
        optional: false,
        description: "Primary Terraria multiplayer port",
      },
      {
        key: "game-udp",
        protocol: "udp",
        defaultPort: 7777,
        optional: true,
        riskWarning: "Optional UDP auxiliary connection port.",
      },
    ],
  },
  {
    id: "l4d2",
    name: "Left 4 Dead 2",
    supportLevel: "experimental",
    tagline: "Source engine co-op campaign dedicated server",
    engineFamily: "Source NetChannel UDP",
    mappings: [
      {
        key: "game",
        protocol: "udp",
        defaultPort: 27015,
        optional: false,
        description: "Primary Source engine game port",
      },
      {
        key: "rcon",
        protocol: "tcp",
        defaultPort: 27015,
        optional: true,
        riskWarning: "RCON provides remote server console access.",
      },
      {
        key: "sourcetv",
        protocol: "udp",
        defaultPort: 27020,
        optional: true,
        riskWarning: "SourceTV spectator broadcast stream.",
      },
      {
        key: "client",
        protocol: "udp",
        defaultPort: 27005,
        optional: true,
        riskWarning: "Client communication port.",
      },
    ],
  },
  {
    id: "l4d1",
    name: "Left 4 Dead",
    supportLevel: "experimental",
    tagline: "Source engine classic co-op server",
    engineFamily: "Source NetChannel UDP",
    mappings: [
      {
        key: "game",
        protocol: "udp",
        defaultPort: 27015,
        optional: false,
        description: "Primary Source engine game port",
      },
    ],
  },
];

// Sensitive local ports deny-list matching contracts.md §13 and target_policy.rs
const SENSITIVE_LOCAL_PORTS = new Set([
  22, 23, 25, 53, 80, 88, 110, 135, 137, 138, 139, 143, 389, 443, 445, 636, 1433,
  2049, 2375, 3306, 3389, 5432, 5900, 5985, 5986, 6379, 6443, 27017,
]);

// Coherent, reconcilable session telemetry dataset (app-screens.md §10.2, PRIV-002)
const ACTIVE_GUEST_FLOWS = [
  {
    flowId: "flw_01J9K8A2",
    guestSubnet: "73.162.48.0/24",
    mappingKey: "game",
    protocol: "TCP",
    localTarget: "127.0.0.1:25565",
    rttMs: 28,
    egressMb: 184.2,
    duration: "42m 18s",
    status: "Admitted",
  },
  {
    flowId: "flw_01J9K8F9",
    guestSubnet: "86.14.119.0/24",
    mappingKey: "game",
    protocol: "TCP",
    localTarget: "127.0.0.1:25565",
    rttMs: 31,
    egressMb: 142.6,
    duration: "36m 04s",
    status: "Admitted",
  },
  {
    flowId: "flw_01J9K9C4",
    guestSubnet: "174.91.204.0/24",
    mappingKey: "game",
    protocol: "TCP",
    localTarget: "127.0.0.1:25565",
    rttMs: 25,
    egressMb: 86.0,
    duration: "19m 51s",
    status: "Admitted",
  },
];

const SESSION_SUMMARY = {
  sessionEgressMb: 412.8, // 184.2 + 142.6 + 86.0 === 412.8 MB
  monthlyUsedGb: 4.12,
  monthlyQuotaGb: 100.0,
  meanRttMs: 28,
  relayRegion: "fra1 (Frankfurt)",
  updatedAt: "14:22:08 UTC",
};

// Reconcile guest flow egress sum against session total (app-screens.md §10.2)
const computedFlowEgressMb = Number(
  ACTIVE_GUEST_FLOWS.reduce((sum, r) => sum + r.egressMb, 0).toFixed(1)
);
console.assert(
  Math.abs(computedFlowEgressMb - SESSION_SUMMARY.sessionEgressMb) < 0.01,
  "Flow egress sum must reconcile with SESSION_SUMMARY.sessionEgressMb"
);

const TELEMETRY_SERIES = [
  { t: "13:30", throughputMbps: 0.42, baselineMbps: 0.38, rttMs: 26 },
  { t: "13:35", throughputMbps: 0.68, baselineMbps: 0.52, rttMs: 27 },
  { t: "13:40", throughputMbps: 0.94, baselineMbps: 0.71, rttMs: 27 },
  { t: "13:45", throughputMbps: 1.28, baselineMbps: 0.85, rttMs: 28 },
  { t: "13:50", throughputMbps: 1.36, baselineMbps: 0.92, rttMs: 28 },
  { t: "13:55", throughputMbps: 1.52, baselineMbps: 1.04, rttMs: 29 },
  { t: "14:00", throughputMbps: 1.64, baselineMbps: 1.12, rttMs: 28 },
  { t: "14:05", throughputMbps: 2.14, baselineMbps: 1.18, rttMs: 31 },
  { t: "14:10", throughputMbps: 1.92, baselineMbps: 1.22, rttMs: 29 },
  { t: "14:15", throughputMbps: 1.84, baselineMbps: 1.19, rttMs: 28 },
  { t: "14:20", throughputMbps: 1.78, baselineMbps: 1.15, rttMs: 28 },
  { t: "14:25", throughputMbps: 1.86, baselineMbps: 1.16, rttMs: 28 },
];

// Initialize mapping config from a profile
function createInitialMappingConfigs(profile) {
  const configs = {};
  profile.mappings.forEach((m) => {
    configs[m.key] = {
      host: "127.0.0.1",
      port: m.defaultPort,
      // Mandatory mappings enabled; optional mappings disabled by default (GAME-008)
      enabled: !m.optional,
      confirmNonLoopback: false,
    };
  });
  return configs;
}

// App State - defaults to "dashboard" so the landing screen is the working tool
const state = {
  currentView: "dashboard", // "dashboard" | "choose-game" | "local-config" | "onboarding" | "signin-handoff"
  theme: localStorage.getItem("gtt-theme") || "system",
  selectedProfile: SHIPPED_PROFILES[0],
  mappingConfigs: createInitialMappingConfigs(SHIPPED_PROFILES[0]),
  validationErrors: {},
  probeResults: {
    game: { status: "reachable", latencyMs: 1 },
  },
  // 3-Part Status Machine (FR-010, UX-002)
  tunnelState: "Connected", // "Not running" | "Connecting" | "Connected"
  localServerState: "Reachable", // "Reachable" | "Not reachable"
  playersSeenCount: ACTIVE_GUEST_FLOWS.length,
  publicAddress: "fra1.relay.gtt.gg:25565",
  isStopModalOpen: false,
  chartMetric: "throughput", // "throughput" | "rtt"
  filterQuery: "",
};

// Target Policy & Validation Functions (contracts.md §13, FR-003, SEC-006, GAME-009)
function isLoopbackHost(host) {
  const trimmed = host.trim().toLowerCase();
  if (trimmed === "localhost") return true;
  if (trimmed === "::1" || trimmed === "0:0:0:0:0:0:0:1") return true;
  if (trimmed.startsWith("127.")) return true;
  return false;
}

function classifyProhibitedCategory(host) {
  const trimmed = host.trim().toLowerCase();
  if (trimmed === "0.0.0.0" || trimmed === "::" || trimmed === "::ffff:0.0.0.0") {
    return "unspecified";
  }
  if (trimmed === "255.255.255.255" || trimmed === "::ffff:255.255.255.255") {
    return "broadcast";
  }
  if (
    trimmed.startsWith("169.254.") ||
    trimmed.startsWith("::ffff:169.254.") ||
    trimmed.startsWith("fe80:") ||
    trimmed.startsWith("febf:")
  ) {
    return "link-local";
  }
  if (
    trimmed.startsWith("224.") ||
    trimmed.startsWith("239.") ||
    trimmed.startsWith("ff02:") ||
    trimmed.startsWith("ff00:")
  ) {
    return "multicast";
  }
  return null;
}

function isValidHostString(host) {
  const trimmed = host.trim().toLowerCase();
  if (trimmed === "localhost") return true;
  const ipv4Match = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(trimmed);
  if (ipv4Match) {
    const parts = [Number(ipv4Match[1]), Number(ipv4Match[2]), Number(ipv4Match[3]), Number(ipv4Match[4])];
    return parts.every((p) => p >= 0 && p <= 255);
  }
  if (trimmed.includes(":") && /^[0-9a-fA-F:.]+$/.test(trimmed)) {
    return true;
  }
  return false;
}

function validateSingleMapping(mapping, config) {
  const errors = {};
  if (!config.enabled) return errors;

  // Port validation (GAME-009)
  const portStr = String(config.port).trim();
  const portNum = Number(portStr);
  if (!portStr || isNaN(portNum) || !Number.isInteger(portNum)) {
    errors.port = "Enter a valid integer port.";
  } else if (portNum === 0) {
    errors.port = "Port cannot be 0. Enter a port between 1 and 65535.";
  } else if (portNum < 1 || portNum > 65535) {
    errors.port = "Port exceeds valid range (1 - 65535).";
  } else if (SENSITIVE_LOCAL_PORTS.has(portNum) && portNum !== mapping.defaultPort) {
    errors.port = `Port ${portNum} is restricted for security (common system service).`;
  }

  // Host validation (FR-003, SEC-006)
  const hostStr = (config.host || "").trim();
  if (!hostStr) {
    errors.host = "Host address is required.";
  } else if (!isValidHostString(hostStr)) {
    errors.host = "Host must be a valid IP address or 'localhost'.";
  } else {
    const prohibitedCat = classifyProhibitedCategory(hostStr);
    if (prohibitedCat) {
      errors.host = `Prohibited address category (${prohibitedCat}). Use 127.0.0.1 or confirm a LAN IP.`;
    } else if (!isLoopbackHost(hostStr)) {
      if (!config.confirmNonLoopback) {
        errors.confirmation = "Non-loopback address requires explicit confirmation.";
      }
    }
  }

  return errors;
}

function validateAllMappings(profile) {
  const allErrors = {};
  let hasEnabled = false;

  profile.mappings.forEach((m) => {
    const cfg = state.mappingConfigs[m.key];
    if (cfg && cfg.enabled) {
      hasEnabled = true;
      const errs = validateSingleMapping(m, cfg);
      if (Object.keys(errs).length > 0) {
        allErrors[m.key] = errs;
      }
    }
  });

  if (!hasEnabled) {
    allErrors._general = "At least one mapping must be enabled.";
  }

  state.validationErrors = allErrors;
  return Object.keys(allErrors).length === 0;
}

// Clean SVG Chart Builder (app-screens.md §7 & assets/charts.js)
function buildTelemetryChartSvg(metric) {
  const width = 680;
  const height = 190;
  const pad = { top: 16, right: 16, bottom: 28, left: 44 };
  const plotW = width - pad.left - pad.right;
  const plotH = height - pad.top - pad.bottom;
  const n = TELEMETRY_SERIES.length;

  const isThroughput = metric === "throughput";
  const yTicks = isThroughput ? [0, 0.8, 1.6, 2.4] : [0, 20, 40, 60];
  const yMax = yTicks[yTicks.length - 1];
  const formatY = isThroughput ? (v) => v.toFixed(1) : (v) => `${v}`;

  const xAt = (i) => pad.left + (i / (n - 1)) * plotW;
  const yAt = (v) => pad.top + plotH - (v / yMax) * plotH;

  const gridLines = yTicks
    .map((v) => {
      const y = yAt(v).toFixed(1);
      return `
        <line x1="${pad.left}" y1="${y}" x2="${width - pad.right}" y2="${y}" stroke="var(--grid)" stroke-width="1"/>
        <text x="${pad.left - 8}" y="${Number(y) + 4}" text-anchor="end" fill="var(--axis-ink)" font-size="11">${formatY(v)}</text>
      `;
    })
    .join("");

  const labelStep = 2;
  const xLabels = TELEMETRY_SERIES.map((d, i) => {
    if (i % labelStep !== 0 && i !== n - 1) return "";
    return `<text x="${xAt(i).toFixed(1)}" y="${height - 6}" text-anchor="middle" fill="var(--axis-ink)" font-size="11">${d.t}</text>`;
  }).join("");

  const primaryValues = TELEMETRY_SERIES.map((d) => (isThroughput ? d.throughputMbps : d.rttMs));
  const primaryPts = primaryValues.map((v, i) => `${xAt(i).toFixed(1)},${yAt(v).toFixed(1)}`).join(" ");

  let secondaryLine = "";
  if (isThroughput) {
    const basePts = TELEMETRY_SERIES.map((d, i) => `${xAt(i).toFixed(1)},${yAt(d.baselineMbps).toFixed(1)}`).join(" ");
    secondaryLine = `<polyline fill="none" stroke="var(--chart-muted)" stroke-width="1.5" stroke-dasharray="3 3" stroke-linejoin="round" stroke-linecap="round" points="${basePts}"/>`;
  } else {
    const sloY = yAt(50).toFixed(1);
    secondaryLine = `
      <line x1="${pad.left}" y1="${sloY}" x2="${width - pad.right}" y2="${sloY}" stroke="var(--chart-3)" stroke-width="1" stroke-dasharray="4 4"/>
      <text x="${width - pad.right - 4}" y="${Number(sloY) - 5}" text-anchor="end" fill="var(--axis-ink)" font-size="11">SLO ceiling 50 ms</text>
    `;
  }

  // Peak annotation at index 7 (14:05)
  const peakIdx = 7;
  const px = xAt(peakIdx).toFixed(1);
  const py = yAt(primaryValues[peakIdx]).toFixed(1);
  const noteLabel = isThroughput ? "3 guests joined 14:05 (2.14 Mbps)" : "Join burst 14:05 (31 ms)";

  return `
    <svg class="chart" viewBox="0 0 ${width} ${height}" role="img" aria-label="${isThroughput ? "Relay throughput in Mbps" : "Relay round-trip latency in ms"}">
      ${gridLines}
      ${xLabels}
      ${secondaryLine}
      <polyline fill="none" stroke="var(--chart-1)" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" points="${primaryPts}"/>
      <circle cx="${px}" cy="${py}" r="3.5" fill="var(--surface)" stroke="var(--chart-1)" stroke-width="2"/>
      <text x="${px}" y="${Math.max(12, Number(py) - 8)}" text-anchor="middle" fill="var(--ink)" font-size="11" font-weight="600">${noteLabel}</text>
    </svg>
  `;
}

// Theme Management (UX-007)
function applyTheme(theme) {
  state.theme = theme;
  localStorage.setItem("gtt-theme", theme);
  const root = document.documentElement;
  if (theme === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", theme);
  }
  const select = document.getElementById("theme-select");
  if (select) select.value = theme;
}

// Navigation
function navigateTo(view) {
  state.currentView = view;
  closeMobileDrawer();
  render();
}

function closeMobileDrawer() {
  document.getElementById("app-sidebar")?.classList.remove("is-open");
  document.getElementById("sidebar-scrim")?.classList.remove("is-open");
}

// Toast helper
function showToast(message) {
  const container = document.getElementById("toast-container");
  if (!container) return;
  const toast = document.createElement("div");
  toast.className = "toast";
  toast.setAttribute("role", "status");
  toast.textContent = message;
  container.appendChild(toast);
  setTimeout(() => {
    toast.remove();
  }, 3000);
}

// Render Controller
function render() {
  const mainContent = document.getElementById("main-content");
  const viewTitle = document.getElementById("view-title");
  const topbarScope = document.getElementById("topbar-scope");
  if (!mainContent) return;

  const primaryPort =
    state.mappingConfigs[state.selectedProfile.mappings[0].key]?.port ||
    state.selectedProfile.mappings[0].defaultPort;

  if (topbarScope) {
    topbarScope.textContent = `${state.selectedProfile.name} · 127.0.0.1:${primaryPort}`;
  }

  const enabledCount = state.selectedProfile.mappings.filter(
    (m) => state.mappingConfigs[m.key]?.enabled
  ).length;
  const navMappingsBadge = document.getElementById("nav-badge-mappings");
  if (navMappingsBadge) {
    navMappingsBadge.textContent = `${enabledCount}/${state.selectedProfile.mappings.length}`;
  }
  const navDashBadge = document.getElementById("nav-badge-dashboard");
  if (navDashBadge) {
    navDashBadge.textContent = state.tunnelState === "Connected" ? "LIVE" : "IDLE";
  }

  if (state.currentView === "onboarding") {
    if (viewTitle) viewTitle.textContent = "Welcome & Privacy";
    mainContent.innerHTML = renderOnboardingView();
    bindOnboardingEvents();
  } else if (state.currentView === "signin-handoff") {
    if (viewTitle) viewTitle.textContent = "Browser Sign-In Handoff";
    mainContent.innerHTML = renderSignInHandoffView();
    bindSignInHandoffEvents();
  } else if (state.currentView === "choose-game") {
    if (viewTitle) viewTitle.textContent = "Choose Game Profile";
    mainContent.innerHTML = renderChooseGameView();
    bindChooseGameEvents();
  } else if (state.currentView === "local-config") {
    if (viewTitle) viewTitle.textContent = `Configure Server · ${state.selectedProfile.name}`;
    mainContent.innerHTML = renderLocalConfigView();
    bindLocalConfigEvents();
  } else {
    if (viewTitle) viewTitle.textContent = "Tunnel Dashboard";
    mainContent.innerHTML = renderDashboardView();
    bindDashboardEvents();
  }

  updateSidebarNav();
}

function updateSidebarNav() {
  document.querySelectorAll(".nav-item").forEach((el) => {
    const target = el.getAttribute("data-target");
    if (target === state.currentView) {
      el.classList.add("active");
    } else {
      el.classList.remove("active");
    }
  });
}

// S1: Welcome & Onboarding View (FR-001, UX-001, PRIV-005, SEC-011)
function renderOnboardingView() {
  return `
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">Welcome to Game Tunnel Tooling</h2>
        <p class="sheet-subtitle">
          Host dedicated game servers on your Windows PC and share them through an authenticated QUIC relay without router port-forwarding.
        </p>
      </div>
      <div class="sheet-header__actions">
        <button id="btn-choose-game" class="gtt-btn gtt-btn-secondary">
          Choose a Game to Host
        </button>
        <button id="btn-start-signin" class="gtt-btn gtt-btn-primary">
          Sign In with Browser
        </button>
      </div>
    </div>

    <div class="workspace-grid">
      <div class="panel col-7">
        <div class="panel__head">
          <h3 class="panel__title">Encryption &amp; Zero-Inspection Guarantee</h3>
          <span class="pill pill--ok">QUIC / TLS 1.3 Verified</span>
        </div>

        <!-- Verbatim Encryption & Privacy Disclosure (docs/privacy-notice.md, PRIV-005, SEC-011) -->
        <div class="disclosure-callout" role="note" aria-label="Privacy and encryption disclosure">
          <strong>Privacy &amp; Encryption Disclosure:</strong><br>
          Your game traffic is encrypted between your PC and our relay servers. Because the relay routes incoming game packets directly to your computer without storing them, network traffic is visible in transit to the relay for routing purposes, but game payloads are never inspected, modified, or written to disk. We do not inspect game content.
        </div>

        <p style="color: var(--muted); font-size: 14px;">
          Signing in uses your default system browser via RFC 8252 loopback OAuth with PKCE. Passwords and refresh tokens are never typed into this window or written to plaintext files.
        </p>
      </div>

      <div class="panel panel--flush col-5">
        <div class="panel__head">
          <h3 class="panel__title">Architectural Boundaries</h3>
          <span class="panel__sub">Enforced in Rust Engine</span>
        </div>
        <div class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Invariant</th>
                <th>Enforcement Rule</th>
                <th class="num">Status</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td><code data-ui>INV-1</code></td>
                <td>Control plane never receives local IP/port</td>
                <td class="num"><span class="pill pill--ok">Locked</span></td>
              </tr>
              <tr>
                <td><code data-ui>SEC-006</code></td>
                <td>29 sensitive system ports denied by default</td>
                <td class="num"><span class="pill pill--ok">Active</span></td>
              </tr>
              <tr>
                <td><code data-ui>PRIV-002</code></td>
                <td>Guest IPs masked to <code data-ui>/24</code> &amp; <code data-ui>/48</code></td>
                <td class="num"><span class="pill pill--ok">Active</span></td>
              </tr>
              <tr>
                <td><code data-ui>GAME-008</code></td>
                <td>Optional RCON/Query ports off by default</td>
                <td class="num"><span class="pill pill--ok">Active</span></td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  `;
}

function bindOnboardingEvents() {
  document.getElementById("btn-start-signin")?.addEventListener("click", () => {
    navigateTo("signin-handoff");
  });
  document.getElementById("btn-choose-game")?.addEventListener("click", () => {
    navigateTo("choose-game");
  });
}

// S2: Sign-In Handoff View (FR-001)
function renderSignInHandoffView() {
  return `
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">Complete Sign-In in Your Browser</h2>
        <p class="sheet-subtitle">
          Waiting for RFC 8252 loopback callback on <code data-ui>127.0.0.1</code> with PKCE code verifier.
        </p>
      </div>
      <div class="sheet-header__actions">
        <button id="btn-cancel-auth" class="gtt-btn gtt-btn-secondary">
          Cancel
        </button>
        <button id="btn-simulate-auth-success" class="gtt-btn gtt-btn-primary">
          Complete Sign-In
        </button>
      </div>
    </div>

    <div class="panel">
      <div class="panel__head">
        <h3 class="panel__title">Browser Authorization Session</h3>
        <span class="pill pill--info">Awaiting Callback</span>
      </div>
      <p style="color: var(--muted);">
        We opened your system web browser to complete authentication. Once authorized, your short-lived session credential is stored in Windows Credential Manager (<code data-ui>CredWriteW</code>) and this console updates automatically.
      </p>
    </div>
  `;
}

function bindSignInHandoffEvents() {
  document.getElementById("btn-simulate-auth-success")?.addEventListener("click", () => {
    showToast("Authentication successful");
    navigateTo("choose-game");
  });
  document.getElementById("btn-cancel-auth")?.addEventListener("click", () => {
    navigateTo("onboarding");
  });
}

// S4: Choose Game Screen (FR-002, GAME-006)
function renderChooseGameView() {
  const query = (state.filterQuery || "").trim().toLowerCase();
  const visibleProfiles = SHIPPED_PROFILES.filter(
    (p) =>
      !query ||
      p.name.toLowerCase().includes(query) ||
      p.id.toLowerCase().includes(query) ||
      p.tagline.toLowerCase().includes(query)
  );
  const selected = state.selectedProfile;

  return `
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">Select a Game Profile</h2>
        <p class="sheet-subtitle">
          Five official profiles shipped from <code data-ui>profiles/*.json</code>. Optional admin/query mappings remain off by default (<code data-ui>GAME-008</code>).
        </p>
      </div>
      <div class="sheet-header__actions">
        <button id="btn-proceed-to-config" class="gtt-btn gtt-btn-primary">
          Configure ${selected.name}
        </button>
      </div>
    </div>

    <div class="workspace-grid">
      <div class="col-8">
        <div class="game-profile-grid" role="list" aria-label="Game profiles list">
          ${visibleProfiles
            .map((p) => {
              const isSelected = p.id === state.selectedProfile.id;
              const badgeClass = p.supportLevel === "verified" ? "verified" : "experimental";
              const badgeText = p.supportLevel === "verified" ? "Tier 1 Verified" : "Experimental";
              const defaultPortSummary = p.mappings
                .map((m) => `${m.protocol.toUpperCase()} ${m.defaultPort}${m.optional ? " (opt)" : ""}`)
                .join(" · ");

              return `
                <div class="game-profile-card ${isSelected ? "selected" : ""}"
                     data-profile-id="${p.id}"
                     role="listitem"
                     tabindex="0"
                     aria-label="${p.name}, ${badgeText}">
                  <div class="game-card-header">
                    <span class="game-card-title">${p.name}</span>
                    <span class="support-badge ${badgeClass}">${badgeText}</span>
                  </div>
                  <div class="game-card-ports">${defaultPortSummary}</div>
                  <p style="font-size: 13px; color: var(--muted); flex-grow: 1;">
                    ${p.tagline}
                  </p>
                  <button class="gtt-btn ${isSelected ? "gtt-btn-primary" : "gtt-btn-secondary"} btn-select-game"
                          data-profile-id="${p.id}"
                          style="width: 100%;">
                    ${isSelected ? "Selected · Edit Mappings" : "Select Profile"}
                  </button>
                </div>
              `;
            })
            .join("")}
        </div>
      </div>

      <div class="panel panel--flush col-4">
        <div class="panel__head">
          <h3 class="panel__title">${selected.name}</h3>
          <span class="pill ${selected.supportLevel === "verified" ? "pill--ok" : "pill--warn"}">
            ${selected.supportLevel === "verified" ? "L2 Real-Client Verified" : "L0/L1 Experimental"}
          </span>
        </div>
        <div class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Key</th>
                <th>Proto</th>
                <th class="num">Default Port</th>
                <th class="num">Default State</th>
              </tr>
            </thead>
            <tbody>
              ${selected.mappings
                .map(
                  (m) => `
                <tr>
                  <td><code data-ui>${m.key}</code></td>
                  <td>${m.protocol.toUpperCase()}</td>
                  <td class="num">${m.defaultPort}</td>
                  <td class="num">
                    <span class="pill ${m.optional ? "pill--idle" : "pill--ok"}">
                      ${m.optional ? "Off (Optional)" : "Enabled"}
                    </span>
                  </td>
                </tr>
              `
                )
                .join("")}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  `;
}

function bindChooseGameEvents() {
  document.getElementById("btn-proceed-to-config")?.addEventListener("click", () => {
    navigateTo("local-config");
  });

  document.querySelectorAll(".game-profile-card").forEach((card) => {
    const pid = card.getAttribute("data-profile-id");
    const selectProfile = () => {
      const found = SHIPPED_PROFILES.find((p) => p.id === pid);
      if (found) {
        state.selectedProfile = found;
        state.mappingConfigs = createInitialMappingConfigs(found);
        state.validationErrors = {};
        state.probeResults = {};
        state.publicAddress = `fra1.relay.gtt.gg:${found.mappings[0].defaultPort}`;
        navigateTo("local-config");
      }
    };

    card.addEventListener("click", selectProfile);
    card.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        selectProfile();
      }
    });
  });
}

// S5: Local Server Configuration Screen (FR-003, GAME-009, GAME-008, contracts.md §13)
function renderLocalConfigView() {
  const profile = state.selectedProfile;
  const generalError = state.validationErrors._general;

  return `
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">${profile.name} · Local Server Configuration</h2>
        <p class="sheet-subtitle">
          Configure local target addresses and custom ports (<code data-ui>GAME-009</code>). Local targets stay strictly inside your host engine (<code data-ui>INV-1</code>).
        </p>
      </div>
      <div class="sheet-header__actions">
        <button type="button" id="btn-back-to-games" class="gtt-btn gtt-btn-secondary">
          Change Game
        </button>
        <button type="button" id="btn-probe-targets" class="gtt-btn gtt-btn-secondary">
          Check Server Reachability (L0 Probe)
        </button>
      </div>
    </div>

    ${
      generalError
        ? `<div class="field-error-message" role="alert" style="font-size: 14px;">${generalError}</div>`
        : ""
    }

    <div class="settings-split">
      <!-- Left Column: Target Policy & Invariant Summary -->
      <aside class="settings-aside">
        <div class="panel">
          <div class="panel__head">
            <h3 class="panel__title">Local Target Policy</h3>
            <span class="pill pill--ok">contracts.md §13</span>
          </div>
          <p style="font-size: 13px; color: var(--muted);">
            The host engine validates every enabled mapping before opening local sockets:
          </p>
          <table class="data-table">
            <tbody>
              <tr>
                <td>Loopback default</td>
                <td class="num"><code data-ui>127.0.0.1</code></td>
              </tr>
              <tr>
                <td>Custom port range</td>
                <td class="num">1 – 65535</td>
              </tr>
              <tr>
                <td>Denied system ports</td>
                <td class="num">22, 80, 443, 445, 3389…</td>
              </tr>
              <tr>
                <td>Prohibited ranges</td>
                <td class="num"><code data-ui>0.0.0.0</code>, multicast, link-local</td>
              </tr>
              <tr>
                <td>Control-plane payload</td>
                <td class="num">Profile ID &amp; keys only</td>
              </tr>
            </tbody>
          </table>
        </div>
      </aside>

      <!-- Right Column: Mapping Configuration Cards (FR-003, GAME-009, GAME-008) -->
      <form id="local-config-form" novalidate>
        ${profile.mappings
          .map((m) => {
            const cfg = state.mappingConfigs[m.key] || {
              host: "127.0.0.1",
              port: m.defaultPort,
              enabled: !m.optional,
              confirmNonLoopback: false,
            };
            const errs = state.validationErrors[m.key] || {};
            const isNonLoopback = !isLoopbackHost(cfg.host || "");
            const probe = state.probeResults[m.key];

            let probeBadgeHtml = "";
            if (probe) {
              if (probe.status === "reachable") {
                probeBadgeHtml = `<span class="probe-badge reachable">Reachable (${probe.latencyMs} ms)</span>`;
              } else if (probe.status === "probing") {
                probeBadgeHtml = `<span class="probe-badge probing">Probing...</span>`;
              } else {
                probeBadgeHtml = `<span class="probe-badge not-reachable">Not reachable</span>`;
              }
            }

            return `
              <div class="mapping-config-card" data-mapping-key="${m.key}">
                <div class="mapping-config-header">
                  <div class="mapping-key-title">
                    <span>${m.key.toUpperCase()} Mapping</span>
                    <span class="protocol-pill">${m.protocol.toUpperCase()}</span>
                    ${
                      m.optional
                        ? '<span style="font-size: 12px; color: var(--muted);">(Optional, off by default)</span>'
                        : '<span style="font-size: 12px; color: var(--muted);">(Primary required)</span>'
                    }
                    ${probeBadgeHtml}
                  </div>
                  <label class="checkbox-label">
                    <input type="checkbox"
                           class="mapping-enable-toggle"
                           data-mapping-key="${m.key}"
                           ${cfg.enabled ? "checked" : ""}
                           ${!m.optional ? "disabled" : ""} />
                    <span>${cfg.enabled ? "Enabled" : "Disabled"}</span>
                  </label>
                </div>

                ${
                  m.riskWarning
                    ? `<div class="risk-callout" role="note"><strong>Security notice:</strong> ${m.riskWarning}</div>`
                    : ""
                }

                <div class="mapping-fields-grid" style="${!cfg.enabled ? "opacity: 0.5; pointer-events: none;" : ""}">
                  <!-- Local Host -->
                  <div class="field-group">
                    <label class="field-label" for="host-${m.key}">Local Host Address</label>
                    <input type="text"
                           id="host-${m.key}"
                           class="field-input input-host ${errs.host ? "is-invalid" : ""}"
                           data-mapping-key="${m.key}"
                           value="${cfg.host}"
                           placeholder="127.0.0.1"
                           aria-invalid="${errs.host ? "true" : "false"}"
                           aria-describedby="err-host-${m.key}" />
                    <div id="err-host-${m.key}" class="field-error-message" role="alert">
                      ${errs.host || ""}
                    </div>
                  </div>

                  <!-- Local Port (GAME-009) -->
                  <div class="field-group">
                    <label class="field-label" for="port-${m.key}">Local Port (Default ${m.defaultPort})</label>
                    <input type="number"
                           id="port-${m.key}"
                           class="field-input input-port ${errs.port ? "is-invalid" : ""}"
                           data-mapping-key="${m.key}"
                           value="${cfg.port}"
                           min="1"
                           max="65535"
                           placeholder="${m.defaultPort}"
                           aria-invalid="${errs.port ? "true" : "false"}"
                           aria-describedby="err-port-${m.key}" />
                    <div id="err-port-${m.key}" class="field-error-message" role="alert">
                      ${errs.port || ""}
                    </div>
                  </div>
                </div>

                <!-- Non-Loopback Confirmation Gate (contracts.md §13) -->
                ${
                  cfg.enabled && isNonLoopback
                    ? `
                  <div class="non-loopback-warning" role="region" aria-label="Non-loopback warning">
                    <span style="font-weight: 600; font-size: 13px; color: var(--negative);">
                      Non-loopback address detected (${cfg.host})
                    </span>
                    <label class="checkbox-label">
                      <input type="checkbox"
                             class="checkbox-confirm-non-loopback"
                             data-mapping-key="${m.key}"
                             ${cfg.confirmNonLoopback ? "checked" : ""} />
                      <span>I confirm forwarding traffic to this remote/LAN host</span>
                    </label>
                    ${
                      errs.confirmation
                        ? `<div class="field-error-message" role="alert">${errs.confirmation}</div>`
                        : ""
                    }
                  </div>
                `
                    : ""
                }
              </div>
            `;
          })
          .join("")}

        <div style="display: flex; justify-content: flex-end; gap: 10px; margin-top: 12px;">
          <button type="submit" id="btn-save-and-proceed" class="gtt-btn gtt-btn-primary">
            Save &amp; Open Dashboard
          </button>
        </div>
      </form>
    </div>
  `;
}

function bindLocalConfigEvents() {
  const form = document.getElementById("local-config-form");
  const profile = state.selectedProfile;

  document.getElementById("btn-back-to-games")?.addEventListener("click", () => {
    navigateTo("choose-game");
  });

  document.querySelectorAll(".mapping-enable-toggle").forEach((toggle) => {
    toggle.addEventListener("change", (e) => {
      const key = e.target.getAttribute("data-mapping-key");
      if (state.mappingConfigs[key]) {
        state.mappingConfigs[key].enabled = e.target.checked;
        delete state.validationErrors[key];
        render();
      }
    });
  });

  document.querySelectorAll(".input-host").forEach((input) => {
    const key = input.getAttribute("data-mapping-key");
    const onHostUpdate = () => {
      if (state.mappingConfigs[key]) {
        state.mappingConfigs[key].host = input.value;
        const mapping = profile.mappings.find((m) => m.key === key);
        if (mapping) {
          const errs = validateSingleMapping(mapping, state.mappingConfigs[key]);
          state.validationErrors[key] = { ...(state.validationErrors[key] || {}), ...errs };
          if (!errs.host && state.validationErrors[key]) delete state.validationErrors[key].host;
        }
      }
    };
    input.addEventListener("input", (e) => {
      if (state.mappingConfigs[key]) state.mappingConfigs[key].host = e.target.value;
    });
    input.addEventListener("blur", () => {
      onHostUpdate();
      render();
    });
  });

  document.querySelectorAll(".input-port").forEach((input) => {
    const key = input.getAttribute("data-mapping-key");
    const onPortUpdate = () => {
      if (state.mappingConfigs[key]) {
        state.mappingConfigs[key].port = input.value;
        const mapping = profile.mappings.find((m) => m.key === key);
        if (mapping) {
          const errs = validateSingleMapping(mapping, state.mappingConfigs[key]);
          state.validationErrors[key] = { ...(state.validationErrors[key] || {}), ...errs };
          if (!errs.port && state.validationErrors[key]) delete state.validationErrors[key].port;
        }
      }
    };
    input.addEventListener("input", (e) => {
      if (state.mappingConfigs[key]) state.mappingConfigs[key].port = e.target.value;
    });
    input.addEventListener("blur", () => {
      onPortUpdate();
      render();
    });
  });

  document.querySelectorAll(".checkbox-confirm-non-loopback").forEach((cb) => {
    cb.addEventListener("change", (e) => {
      const key = e.target.getAttribute("data-mapping-key");
      if (state.mappingConfigs[key]) {
        state.mappingConfigs[key].confirmNonLoopback = e.target.checked;
        if (state.validationErrors[key]) delete state.validationErrors[key].confirmation;
        render();
      }
    });
  });

  document.getElementById("btn-probe-targets")?.addEventListener("click", () => {
    profile.mappings.forEach((m) => {
      if (state.mappingConfigs[m.key]?.enabled) {
        state.probeResults[m.key] = { status: "probing" };
      }
    });
    render();

    setTimeout(() => {
      profile.mappings.forEach((m) => {
        const cfg = state.mappingConfigs[m.key];
        if (cfg && cfg.enabled) {
          const isValid = !validateSingleMapping(m, cfg).port && !validateSingleMapping(m, cfg).host;
          state.probeResults[m.key] = isValid
            ? { status: "reachable", latencyMs: 1 }
            : { status: "not-reachable" };
        }
      });
      state.localServerState = "Reachable";
      showToast("Local server reachability checked");
      render();
    }, 350);
  });

  form?.addEventListener("submit", (e) => {
    e.preventDefault();
    const isValid = validateAllMappings(profile);

    if (!isValid) {
      render();
      const firstInvalidInput = document.querySelector(".field-input.is-invalid");
      if (firstInvalidInput) {
        firstInvalidInput.focus();
      }
      return;
    }

    const primaryPort = state.mappingConfigs[profile.mappings[0].key]?.port || profile.mappings[0].defaultPort;
    state.publicAddress = `fra1.relay.gtt.gg:${primaryPort}`;
    showToast("Server configuration saved");
    navigateTo("dashboard");
  });
}

// S7: Dashboard View with Divided 4-Cell Status Strip, SVG Telemetry Chart & Reconciled Flows Table
function renderDashboardView() {
  const isTunnelActive = state.tunnelState === "Connected";
  const isTunnelConnecting = state.tunnelState === "Connecting";

  let tunnelBadgeClass = "neutral";
  let tunnelPillClass = "pill--idle";
  if (state.tunnelState === "Connected") {
    tunnelBadgeClass = "ok";
    tunnelPillClass = "pill--ok";
  } else if (state.tunnelState === "Connecting") {
    tunnelBadgeClass = "info";
    tunnelPillClass = "pill--info";
  }

  const serverBadgeClass = state.localServerState === "Reachable" ? "ok" : "error";
  const serverPillClass = state.localServerState === "Reachable" ? "pill--ok" : "pill--err";
  const playersBadgeClass = state.playersSeenCount > 0 ? "ok" : "neutral";
  const playersPillClass = state.playersSeenCount > 0 ? "pill--ok" : "pill--idle";

  const primaryMapping = state.selectedProfile.mappings[0];
  const primaryCfg = state.mappingConfigs[primaryMapping.key] || {
    host: "127.0.0.1",
    port: primaryMapping.defaultPort,
  };

  const query = (state.filterQuery || "").trim().toLowerCase();
  const activeFlows = isTunnelActive
    ? ACTIVE_GUEST_FLOWS.filter(
        (f) =>
          !query ||
          f.flowId.toLowerCase().includes(query) ||
          f.guestSubnet.toLowerCase().includes(query) ||
          f.mappingKey.toLowerCase().includes(query)
      )
    : [];

  const filteredSumMb = activeFlows.reduce((acc, f) => acc + f.egressMb, 0).toFixed(1);

  return `
    <!-- Sheet Header -->
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">${state.selectedProfile.name} · Relay Session</h2>
        <p class="sheet-subtitle">
          Region <span class="num">${SESSION_SUMMARY.relayRegion}</span> · Local target <code data-ui>${primaryCfg.host}:${primaryCfg.port}</code> stays on host (<code data-ui>INV-1</code>)
        </p>
      </div>
      <div class="sheet-header__actions">
        <button id="btn-change-game-dash" class="gtt-btn gtt-btn-secondary">
          Switch Game
        </button>
        <button id="btn-edit-config" class="gtt-btn gtt-btn-secondary">
          Edit Mappings
        </button>
        ${
          !isTunnelActive
            ? `<button id="btn-start-tunnel" class="gtt-btn gtt-btn-primary" ${isTunnelConnecting ? "disabled" : ""}>
                ${isTunnelConnecting ? "Starting..." : "Start Tunnel"}
               </button>`
            : `<button id="btn-stop-tunnel" class="gtt-btn gtt-btn-destructive">
                Stop Tunnel
               </button>`
        }
      </div>
    </div>

    <!-- 3-Part Independent Status Machine + Quota Cell in a Single Divided Strip (UX-002, FR-010, app-screens.md §5.2) -->
    <section class="kpis" role="region" aria-label="Service status summary">
      <!-- 1. Tunnel Connection -->
      <div class="kpi">
        <div class="kpi__label">
          <span>Tunnel Connection</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${tunnelBadgeClass}">
            <span class="status-icon ${tunnelBadgeClass}"></span>
            <span class="kpi__value">${state.tunnelState}</span>
          </div>
        </div>
        <div class="kpi__sub">QUIC v1 · ${SESSION_SUMMARY.meanRttMs} ms RTT to fra1</div>
      </div>

      <!-- 2. Local Server -->
      <div class="kpi">
        <div class="kpi__label">
          <span>Local Server (${primaryCfg.host})</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${serverBadgeClass}">
            <span class="status-icon ${serverBadgeClass}"></span>
            <span class="kpi__value">${state.localServerState}</span>
          </div>
        </div>
        <div class="kpi__sub">${primaryMapping.protocol.toUpperCase()} port ${primaryCfg.port} · 1.1 ms local</div>
      </div>

      <!-- 3. Guest Traffic Observed -->
      <div class="kpi">
        <div class="kpi__label">
          <span>Guest Traffic Observed</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${playersBadgeClass}">
            <span class="status-icon ${playersBadgeClass}"></span>
            <span class="kpi__value">${state.playersSeenCount > 0 ? `${state.playersSeenCount} guests seen` : "No traffic seen"}</span>
          </div>
        </div>
        <div class="kpi__sub">${isTunnelActive ? `${SESSION_SUMMARY.sessionEgressMb} MB across ${state.playersSeenCount} flows` : "Awaiting guest packets"}</div>
      </div>

      <!-- 4. Relay Egress & Quota -->
      <div class="kpi">
        <div class="kpi__label">
          <span>Monthly Relay Quota</span>
        </div>
        <div class="kpi__value-row">
          <span class="kpi__value">${SESSION_SUMMARY.monthlyUsedGb} GB</span>
          <span class="kpi__sub">/ ${SESSION_SUMMARY.monthlyQuotaGb} GB</span>
        </div>
        <div class="kpi__sub kpi__sub--ok">95.88 GB remaining (4.1% used)</div>
      </div>
    </section>

    <!-- End-to-End Encrypted Path Strip & Shareable Public Join Address (FR-007, INV-1) -->
    <section class="path-strip" aria-label="Encrypted tunnel path and join address">
      <div class="path-Hop-chain">
        <span class="path-node"><code data-ui>${primaryCfg.host}:${primaryCfg.port}</code> <span>Local</span></span>
        <span class="path-edge">1.1 ms →</span>
        <span class="path-node"><strong>Host Engine</strong> <code data-ui>INV-1</code></span>
        <span class="path-edge">QUIC 28 ms →</span>
        <span class="path-node"><code data-ui>${state.publicAddress}</code></span>
      </div>

      <div class="copy-field-container">
        <input id="public-address-input"
               class="copy-input"
               readonly
               aria-label="Shareable public tunnel address"
               value="${state.publicAddress}" />
        <button id="btn-copy-address" class="gtt-btn gtt-btn-secondary" aria-label="Copy public address to clipboard">
          Copy Address
        </button>
      </div>
    </section>

    <!-- Main 12-Column Analytical & Configuration Workspace -->
    <div class="workspace-grid">
      <!-- Left 7 Columns: Single Clean SVG Telemetry Line Chart -->
      <div class="panel col-7">
        <div class="panel__head">
          <div>
            <h3 class="panel__title">
              ${state.chartMetric === "throughput" ? "Relay Throughput (Mbps)" : "Relay Round-Trip Latency (ms)"}
            </h3>
            <span class="panel__sub">13:30–14:25 UTC · 5-minute intervals · Peak 2.14 Mbps at 14:05</span>
          </div>
          <div class="tab-strip" role="tablist" aria-label="Telemetry chart metric">
            <button type="button"
                    class="tab-btn ${state.chartMetric === "throughput" ? "active" : ""}"
                    data-metric="throughput"
                    role="tab"
                    aria-selected="${state.chartMetric === "throughput"}">
              Throughput (Mbps)
            </button>
            <button type="button"
                    class="tab-btn ${state.chartMetric === "rtt" ? "active" : ""}"
                    data-metric="rtt"
                    role="tab"
                    aria-selected="${state.chartMetric === "rtt"}">
              RTT Latency (ms)
            </button>
          </div>
        </div>

        <div class="chart-wrap">
          ${buildTelemetryChartSvg(state.chartMetric)}
        </div>

        <div class="chart-legend">
          <div class="legend-items">
            <span class="legend-item">
              <span class="legend-swatch" style="background-color: var(--chart-1);"></span>
              <span>${state.chartMetric === "throughput" ? "Active session (Mbps)" : "Measured QUIC RTT (ms)"}</span>
            </span>
            <span class="legend-item">
              <span class="legend-swatch" style="background-color: ${state.chartMetric === "throughput" ? "var(--chart-muted)" : "var(--chart-3)"};"></span>
              <span>${state.chartMetric === "throughput" ? "Prior session baseline" : "SLO target ceiling (50 ms)"}</span>
            </span>
          </div>
          <span>Zero packet drops in last 55m</span>
        </div>
      </div>

      <!-- Right 5 Columns: Profile Mapping Status Table (GAME-008, GAME-009) -->
      <div class="panel panel--flush col-5">
        <div class="panel__head">
          <div>
            <h3 class="panel__title">Configured Port Mappings</h3>
            <span class="panel__sub">${state.selectedProfile.name}</span>
          </div>
          <button type="button" id="btn-inline-edit-mappings" class="gtt-btn gtt-btn-secondary gtt-btn-sm">
            Configure
          </button>
        </div>
        <div class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Mapping</th>
                <th>Local Target</th>
                <th class="num">L0 Probe</th>
                <th class="num">State</th>
              </tr>
            </thead>
            <tbody>
              ${state.selectedProfile.mappings
                .map((m) => {
                  const cfg = state.mappingConfigs[m.key] || {
                    host: "127.0.0.1",
                    port: m.defaultPort,
                    enabled: !m.optional,
                  };
                  return `
                    <tr>
                      <td>
                        <code data-ui>${m.key}</code>
                        <span style="color: var(--muted); font-size: 12px; margin-left: 4px;">${m.protocol.toUpperCase()}</span>
                      </td>
                      <td><code data-ui>${cfg.host}:${cfg.port}</code></td>
                      <td class="num">${cfg.enabled ? "1.1 ms" : "—"}</td>
                      <td class="num">
                        <span class="pill ${cfg.enabled ? "pill--ok" : "pill--idle"}">
                          ${cfg.enabled ? "Forwarding" : "Off (GAME-008)"}
                        </span>
                      </td>
                    </tr>
                  `;
                })
                .join("")}
            </tbody>
          </table>
        </div>
      </div>

      <!-- Full-Width 12 Columns: Admitted Guest Flows Table (PRIV-002 /24 Masked) -->
      <div class="panel panel--flush col-12">
        <div class="panel__head">
          <div>
            <h3 class="panel__title">Admitted Guest Flows (<code data-ui>/24</code> Masked — PRIV-002)</h3>
            <span class="panel__sub">Raw guest IP addresses are truncated before logging or UI display (<code data-ui>INV-5</code>)</span>
          </div>
          <span class="pill ${activeFlows.length > 0 ? "pill--ok" : "pill--idle"}">
            ${activeFlows.length} Admitted Flows
          </span>
        </div>
        <div class="table-scroll">
          <table class="data-table">
            <thead>
              <tr>
                <th>Flow ID</th>
                <th>Guest Source Subnet</th>
                <th>Mapping</th>
                <th>Local Target</th>
                <th class="num">RTT</th>
                <th class="num">Session Egress</th>
                <th class="num">Duration</th>
                <th class="num">Admission</th>
              </tr>
            </thead>
            <tbody>
              ${
                activeFlows.length > 0
                  ? activeFlows
                      .map(
                        (f) => `
                    <tr>
                      <td><code data-ui>${f.flowId}</code></td>
                      <td><code data-ui>${f.guestSubnet}</code></td>
                      <td><code data-ui>${f.mappingKey}</code> (${f.protocol})</td>
                      <td><code data-ui>${primaryCfg.host}:${primaryCfg.port}</code></td>
                      <td class="num">${f.rttMs} ms</td>
                      <td class="num">${f.egressMb.toFixed(1)} MB</td>
                      <td class="num">${f.duration}</td>
                      <td class="num"><span class="pill pill--ok">${f.status}</span></td>
                    </tr>
                  `
                      )
                      .join("")
                  : `
                    <tr>
                      <td colspan="8" style="text-align: center; color: var(--muted); padding: 20px;">
                        ${isTunnelActive ? "No flows match the current filter." : "Tunnel is stopped. Click Start Tunnel to admit guest connections."}
                      </td>
                    </tr>
                  `
              }
            </tbody>
            <tfoot>
              <tr>
                <td colspan="4">Reconciled Session Total (${activeFlows.length} active guest flows)</td>
                <td class="num">${SESSION_SUMMARY.meanRttMs} ms avg</td>
                <td class="num">${filteredSumMb} MB</td>
                <td class="num">42m 18s peak</td>
                <td class="num">100% admitted</td>
              </tr>
            </tfoot>
          </table>
        </div>
      </div>
    </div>

    <!-- Confirmation Modal when stopping tunnel with active players (UX-006) -->
    <div id="stop-confirmation-modal" class="modal-overlay ${state.isStopModalOpen ? "active" : ""}">
      <div class="modal-card" role="dialog" aria-modal="true" aria-labelledby="modal-title">
        <h3 id="modal-title" class="gtt-card-title">Disconnect Active Players?</h3>
        <p style="color: var(--muted); line-height: 1.5;">
          There are currently <strong>${state.playersSeenCount} active guests</strong> connected to your tunnel. Stopping now will immediately disconnect them from the game.
        </p>
        <div class="modal-actions">
          <button id="btn-cancel-stop" class="gtt-btn gtt-btn-secondary">
            Keep Running
          </button>
          <button id="btn-confirm-stop" class="gtt-btn gtt-btn-destructive">
            Stop &amp; Disconnect
          </button>
        </div>
      </div>
    </div>
  `;
}

function bindDashboardEvents() {
  document.getElementById("btn-change-game-dash")?.addEventListener("click", () => {
    navigateTo("choose-game");
  });

  document.getElementById("btn-edit-config")?.addEventListener("click", () => {
    navigateTo("local-config");
  });

  document.getElementById("btn-inline-edit-mappings")?.addEventListener("click", () => {
    navigateTo("local-config");
  });

  document.querySelectorAll(".tab-btn[data-metric]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const m = btn.getAttribute("data-metric");
      if (m) {
        state.chartMetric = m;
        render();
      }
    });
  });

  // Start tunnel
  document.getElementById("btn-start-tunnel")?.addEventListener("click", () => {
    state.tunnelState = "Connecting";
    render();
    setTimeout(() => {
      state.tunnelState = "Connected";
      state.localServerState = "Reachable";
      state.playersSeenCount = ACTIVE_GUEST_FLOWS.length;
      showToast("Tunnel started successfully");
      render();
    }, 600);
  });

  // Stop tunnel (enforces UX-006 confirmation if players seen)
  document.getElementById("btn-stop-tunnel")?.addEventListener("click", () => {
    if (state.playersSeenCount > 0) {
      state.isStopModalOpen = true;
      render();
    } else {
      executeStopTunnel();
    }
  });

  document.getElementById("btn-cancel-stop")?.addEventListener("click", () => {
    state.isStopModalOpen = false;
    render();
  });

  document.getElementById("btn-confirm-stop")?.addEventListener("click", () => {
    state.isStopModalOpen = false;
    executeStopTunnel();
  });

  document.getElementById("btn-copy-address")?.addEventListener("click", () => {
    const input = document.getElementById("public-address-input");
    if (input) {
      navigator.clipboard?.writeText(input.value);
      showToast("Public address copied to clipboard!");
    }
  });
}

function executeStopTunnel() {
  state.tunnelState = "Not running";
  state.playersSeenCount = 0;
  showToast("Tunnel stopped");
  render();
}

// Global initialization
window.addEventListener("DOMContentLoaded", () => {
  const themeSelect = document.getElementById("theme-select");
  if (themeSelect) {
    themeSelect.value = state.theme;
    themeSelect.addEventListener("change", (e) => {
      applyTheme(e.target.value);
    });
  }
  applyTheme(state.theme);

  document.querySelectorAll(".nav-item").forEach((el) => {
    el.addEventListener("click", () => {
      const target = el.getAttribute("data-target");
      if (target) navigateTo(target);
    });
  });

  // Mobile drawer toggle
  document.getElementById("btn-menu-toggle")?.addEventListener("click", () => {
    document.getElementById("app-sidebar")?.classList.toggle("is-open");
    document.getElementById("sidebar-scrim")?.classList.toggle("is-open");
  });
  document.getElementById("sidebar-scrim")?.addEventListener("click", closeMobileDrawer);

  // Global search filter
  const searchInput = document.getElementById("global-filter-input");
  if (searchInput) {
    searchInput.addEventListener("input", (e) => {
      state.filterQuery = e.target.value;
      render();
    });
  }

  // Topbar refresh button
  document.getElementById("btn-refresh-telemetry")?.addEventListener("click", () => {
    const stamp = document.getElementById("freshness-stamp");
    if (stamp) {
      stamp.textContent = "Updated just now";
    }
    showToast("Telemetry refreshed");
  });

  render();
});

// Export symbols onto globalThis for Node VM unit testing (tests/game_selection_and_config_test.js)
Object.assign(globalThis, {
  SHIPPED_PROFILES,
  SENSITIVE_LOCAL_PORTS,
  createInitialMappingConfigs,
  isLoopbackHost,
  classifyProhibitedCategory,
  isValidHostString,
  validateSingleMapping,
  validateAllMappings,
  state,
});
