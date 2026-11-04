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
  currentView: "dashboard", // "dashboard" | "choose-game" | "local-config" | "invitations" | "troubleshooter" | "onboarding" | "signin-handoff"
  theme: localStorage.getItem("gtt-theme") || "system",
  isSignedIn: true,
  selectedProfile: SHIPPED_PROFILES[0],
  mappingConfigs: createInitialMappingConfigs(SHIPPED_PROFILES[0]),
  validationErrors: {},
  probeResults: {
    game: { status: "reachable", latencyMs: 1 },
  },
  // 3-Part Status Machine (FR-010, UX-002)
  tunnelState: "Connected", // "Not running" | "Connecting" | "Connected" | "Degraded" | "Reconnecting" | "Failed"
  localServerState: "Reachable", // "Reachable" | "Not reachable" | "Checking" | "Unknown"
  playersSeenCount: ACTIVE_GUEST_FLOWS.length,
  publicAddress: "fra1.relay.gtt.gg:25565",
  isStopModalOpen: false,
  chartMetric: "throughput", // "throughput" | "rtt"
  filterQuery: "",
  // T-084: Invitations & Guest Admissions (FR-007, FR-008, FR-009, SEC-007)
  // Note: Invitation list NEVER stores plaintext secrets (contracts.md §5.12)
  invitations: [
    {
      id: "inv_01J9K7M1",
      createdAt: "13:30:00 UTC",
      expiresAt: "15:30:00 UTC",
      expiresInSeconds: 7200,
      maxClaims: 4,
      claimCount: 2,
      status: "active", // "active" | "revoked" | "expired" | "exhausted"
    },
    {
      id: "inv_01J9K8B4",
      createdAt: "13:50:00 UTC",
      expiresAt: "14:50:00 UTC",
      expiresInSeconds: 3600,
      maxClaims: 2,
      claimCount: 1,
      status: "active",
    },
  ],
  oneTimeInvitationBanner: null, // { invitationId, secret, claimUrl } — shown once at creation, never stored in list
  guestAdmissions: [
    {
      id: "adm_01J9K8A2",
      invitationId: "inv_01J9K7M1",
      guestSubnet: "73.162.48.0/24",
      admittedAt: "13:39:42 UTC",
      admittedUntil: "15:30:00 UTC",
      status: "active", // "active" | "revoked"
    },
    {
      id: "adm_01J9K8F9",
      invitationId: "inv_01J9K7M1",
      guestSubnet: "86.14.119.0/24",
      admittedAt: "13:46:04 UTC",
      admittedUntil: "15:30:00 UTC",
      status: "active",
    },
    {
      id: "adm_01J9K9C4",
      invitationId: "inv_01J9K8B4",
      guestSubnet: "174.91.204.0/24",
      admittedAt: "14:02:17 UTC",
      admittedUntil: "14:50:00 UTC",
      status: "active",
    },
  ],
  guestClaimPreviewResult: null,
  // T-085: Active error code selected in troubleshooter / banner (FR-015, UX-003)
  selectedErrorCode: "relay_unavailable",
  lastDiagnosticsExport: null,
  // T-086: Accessibility focus tracking (UX-004, DESIGN.md §7 & §9)
  activeFocusedElementId: null,
  // T-088: Managed Server Lifecycle & Provisioning State (SRV-001..SRV-011, DESIGN.md §6 & §8)
  isManagedUpdateModalOpen: false,
  pendingManagedUpdateVersion: null,
  managedServer: {
    profileId: "minecraft-java",
    state: "Not set up", // "Not set up" | "Downloading" | "Verifying" | "Configuring" | "Starting" | "Running" | "Crashed" | "Updating" | "Failed"
    termsAccepted: false,
    eulaUrl: "https://aka.ms/MinecraftEULA",
    currentVersion: "1.21.1",
    availableUpdateVersion: "1.21.3",
    listenAddress: "127.0.0.1:25565",
    progressPercent: 0,
    activeStep: null, // "download" | "verify" | "configure" | "start" | null
    stepStatuses: {
      download: "pending",
      verify: "pending",
      configure: "pending",
      start: "pending",
    },
    cancelSummary: null,
    failureCode: null,
    restartAttempt: 0,
    maxRestartAttempts: 3,
    artifactVerified: false,
    artifactDiscarded: false,
    crashLogExcerpt:
      "[14:21:09] [Server thread/ERROR]: Encountered an unexpected exception\njava.lang.OutOfMemoryError: Java heap space\n  at net.minecraft.server.MinecraftServer.runServer(MinecraftServer.java:682)",
    generatedConfig: {
      serverIp: "127.0.0.1",
      serverPort: 25565,
      onlineMode: true,
      enableRcon: false,
      enableQuery: false,
      whiteList: true,
      motd: "GTT Managed Minecraft Server",
    },
    confirmedInGameCount: null, // SRV-011: null when Server Management Protocol is off; integer when on
    serverManagementProtocolEnabled: false,
    backups: [
      {
        id: "bkp_01J9K5A0",
        label: "Pre-session world snapshot",
        version: "1.21.1",
        createdAt: "13:15:00 UTC",
        sizeMb: 48.4,
      },
    ],
    lastRestoredBackupId: null,
  },
};

// Per-game join instructions shown on Guest Claim Page G1 (user-flows.md §7)
const GAME_JOIN_INSTRUCTIONS = {
  "minecraft-java": "Open Minecraft: Java Edition -> Multiplayer -> Direct Connection and paste the public address.",
  "minecraft-bedrock": "Open Minecraft: Bedrock Edition (Windows PC) -> Play -> Servers -> Add Server with the host and port.",
  terraria: "Open Terraria -> Multiplayer -> Join via IP and enter the public host and port.",
  l4d2: "Open Left 4 Dead 2 developer console (~) and run: connect <host:port>",
  l4d1: "Open Left 4 Dead developer console (~) and run: connect <host:port>",
};

// In-memory server-side simulation vault mapping issued secret -> invitationId (never exposed in list responses)
const invitationSecretVault = new Map();
let invitationSeqCounter = 100;

// Generates >= 128-bit entropy base64url invitation secret (24 base64url chars = 144 bits, SEC-007)
function generateInvitationSecret() {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let token = "gtt_inv_";
  for (let i = 0; i < 24; i++) {
    const idx = Math.floor(Math.random() * alphabet.length);
    token += alphabet[idx];
  }
  return token;
}

// POST /v1/tunnels/{id}/invitations (FR-007, SEC-007, contracts.md §5.11)
function createInvitation(options = {}) {
  const expiresInSeconds = Number(options.expiresInSeconds ?? 3600);
  const maxClaims = Number(options.maxClaims ?? 4);

  if (!Number.isInteger(expiresInSeconds) || expiresInSeconds < 60 || expiresInSeconds > 604800) {
    return {
      ok: false,
      error: {
        code: "validation_failed",
        message: "Expiry must be between 60 seconds (1 minute) and 604800 seconds (7 days).",
      },
    };
  }

  if (!Number.isInteger(maxClaims) || maxClaims < 1 || maxClaims > 32) {
    return {
      ok: false,
      error: {
        code: "validation_failed",
        message: "Maximum claims must be an integer between 1 and 32.",
      },
    };
  }

  if (state.tunnelState === "Failed") {
    return {
      ok: false,
      error: {
        code: "conflict",
        message: "Cannot create an invitation while the tunnel is in Failed state.",
      },
    };
  }

  invitationSeqCounter += 1;
  const invId = `inv_01J9K9_${invitationSeqCounter}`;
  const secret = generateInvitationSecret();

  // Stored list record NEVER includes the secret field (FR-007, contracts.md §5.12)
  const record = {
    id: invId,
    createdAt: "14:22:15 UTC",
    expiresAt: `in ${Math.round(expiresInSeconds / 60)}m`,
    expiresInSeconds,
    maxClaims,
    claimCount: 0,
    status: "active",
  };

  state.invitations.unshift(record);
  invitationSecretVault.set(secret, invId);

  const claimUrl = `https://join.gtt.gg/i#${secret}`;
  state.oneTimeInvitationBanner = {
    invitationId: invId,
    secret,
    claimUrl,
  };

  render();
  return {
    ok: true,
    invitation: { ...record },
    oneTimeSecret: secret,
    claimUrl,
  };
}

// GET /v1/tunnels/{id}/invitations — never returns secrets (FR-007, contracts.md §5.12)
function listInvitations() {
  return state.invitations.map((inv) => {
    const copy = { ...inv };
    delete copy.secret;
    return copy;
  });
}

function dismissOneTimeSecret() {
  state.oneTimeInvitationBanner = null;
  render();
}

// DELETE /v1/invitations/{id} — revokes invitation and cascades to its admissions (FR-009, contracts.md §5.12)
function revokeInvitation(invitationId) {
  const target = state.invitations.find((inv) => inv.id === invitationId);
  if (!target) {
    return {
      ok: false,
      error: { code: "not_found", message: "Invitation not found." },
    };
  }

  target.status = "revoked";
  target.revokedAt = "14:24:00 UTC";

  // Cascade revocation to any guest admissions created by this invitation
  state.guestAdmissions.forEach((adm) => {
    if (adm.invitationId === invitationId) {
      adm.status = "revoked";
    }
  });

  if (state.oneTimeInvitationBanner && state.oneTimeInvitationBanner.invitationId === invitationId) {
    state.oneTimeInvitationBanner = null;
  }

  render();
  return { ok: true, invitation: { ...target } };
}

// DELETE /v1/admissions/{id} (FR-009, contracts.md §5.13)
function revokeAdmission(admissionId) {
  const adm = state.guestAdmissions.find((a) => a.id === admissionId);
  if (!adm) {
    return {
      ok: false,
      error: { code: "not_found", message: "Admission not found." },
    };
  }
  adm.status = "revoked";
  render();
  return { ok: true, admission: { ...adm } };
}

// POST /v1/invitations/claim — Guest claim endpoint (FR-008, contracts.md §6, user-flows.md §7)
// Expired, revoked, exhausted, or unknown secrets all return "invitation_invalid" so a guesser learns nothing.
function claimInvitation(secret, observedGuestSubnet = "198.51.100.0/24") {
  const cleanSecret = String(secret || "").trim();
  const invId = invitationSecretVault.get(cleanSecret);
  if (!invId) {
    const errResp = {
      ok: false,
      error: {
        code: "invitation_invalid",
        message: "This invitation isn't valid.",
      },
    };
    state.guestClaimPreviewResult = errResp;
    render();
    return errResp;
  }

  const inv = state.invitations.find((i) => i.id === invId);
  if (!inv || inv.status !== "active" || inv.claimCount >= inv.maxClaims) {
    if (inv && inv.status === "active" && inv.claimCount >= inv.maxClaims) {
      inv.status = "exhausted";
    }
    const errResp = {
      ok: false,
      error: {
        code: "invitation_invalid",
        message: "This invitation isn't valid.",
      },
    };
    state.guestClaimPreviewResult = errResp;
    render();
    return errResp;
  }

  inv.claimCount += 1;
  if (inv.claimCount >= inv.maxClaims) {
    inv.status = "exhausted";
  }

  const admission = {
    id: `adm_01J9K9_${invitationSeqCounter + inv.claimCount}`,
    invitationId: inv.id,
    guestSubnet: observedGuestSubnet,
    admittedAt: "14:23:10 UTC",
    admittedUntil: inv.expiresAt,
    status: "active",
  };
  state.guestAdmissions.unshift(admission);

  const enabledMappings = state.selectedProfile.mappings
    .filter((m) => state.mappingConfigs[m.key]?.enabled)
    .map((m) => ({
      mapping_key: m.key,
      protocol: m.protocol,
      public_port: state.mappingConfigs[m.key]?.port || m.defaultPort,
    }));

  const successResp = {
    ok: true,
    data: {
      invitation_id: inv.id,
      admission_id: admission.id,
      game_profile_id: state.selectedProfile.id,
      game_name: state.selectedProfile.name,
      public_address: state.publicAddress,
      mappings: enabledMappings,
      instruction: GAME_JOIN_INSTRUCTIONS[state.selectedProfile.id] || GAME_JOIN_INSTRUCTIONS["minecraft-java"],
      admitted_until: inv.expiresAt,
      network_mismatch_tip:
        "Open this invitation link on the same network and device you will use to launch the game.",
    },
  };
  state.guestClaimPreviewResult = successResp;
  render();
  return successResp;
}

// ============================================================================
// T-085: Error Message Catalog & Guided Troubleshooter (FR-015, UX-003, contracts.md §3)
// Every code in contracts.md §3 and every engine failure class maps to a reviewed
// plain-language cause and next action. Never echoes secrets or internal IDs.
// ============================================================================
const REQUIRED_CONTRACT_ERROR_CODES = [
  "unauthenticated",
  "forbidden",
  "not_found",
  "validation_failed",
  "conflict",
  "idempotency_mismatch",
  "rate_limited",
  "quota_exceeded",
  "invitation_invalid",
  "relay_unavailable",
  "client_upgrade_required",
  "internal",
];

const REQUIRED_ENGINE_FAILURE_CLASSES = [
  "udp_blocked",
  "local_server_down",
  "local_server_wrong_bind",
  "optional_port_disabled",
  "token_expired",
  "handshake_failed",
  "grace_period_expired",
  "srv_download_failed",
  "srv_verification_failed",
  "srv_terms_not_accepted",
  "srv_disk_space_insufficient",
  "srv_process_crashed",
  "srv_restart_limit_reached",
];

const MANAGED_SERVER_FAILURE_CODES = [
  "srv_download_failed",
  "srv_verification_failed",
  "srv_terms_not_accepted",
  "srv_disk_space_insufficient",
  "srv_process_crashed",
  "srv_restart_limit_reached",
];

const ERROR_MESSAGE_CATALOG = {
  // 12 Official Contract Error Codes (contracts.md §3)
  unauthenticated: {
    code: "unauthenticated",
    category: "api",
    httpStatus: 401,
    title: "Sign-in required or session expired",
    cause: "Your sign-in session has expired or was signed out on another device.",
    action: "Sign in again through your system web browser to continue hosting.",
    actionLabel: "Sign In Again",
  },
  forbidden: {
    code: "forbidden",
    category: "api",
    httpStatus: 403,
    title: "This account can't create or run tunnels",
    cause: "Your account is currently suspended or restricted from starting relay sessions.",
    action: "Contact support with your request ID if you believe this restriction was applied in error.",
    actionLabel: "Contact Support",
  },
  not_found: {
    code: "not_found",
    category: "api",
    httpStatus: 404,
    title: "Tunnel or invitation no longer exists",
    cause: "The requested tunnel, invitation, or admission was stopped, deleted, or belongs to another account.",
    action: "Refresh your tunnel list or create a new tunnel from the Choose Game screen.",
    actionLabel: "Refresh Workspace",
  },
  validation_failed: {
    code: "validation_failed",
    category: "api",
    httpStatus: 400,
    title: "Configuration settings need attention",
    cause: "One or more fields (such as port range, host address, or invitation expiry) did not pass validation.",
    action: "Review the highlighted fields, enter a valid port (1–65535) or loopback address (127.0.0.1), and save again.",
    actionLabel: "Review Fields",
  },
  conflict: {
    code: "conflict",
    category: "api",
    httpStatus: 409,
    title: "Action conflicts with current tunnel state",
    cause: "The tunnel is already stopping, stopped, or in a state that does not allow this operation right now.",
    action: "Wait for the current state transition to finish, or start a fresh tunnel session.",
    actionLabel: "Reload Tunnel State",
  },
  idempotency_mismatch: {
    code: "idempotency_mismatch",
    category: "api",
    httpStatus: 422,
    title: "Duplicate request had different settings",
    cause: "A retry reused an existing operation key while changing the tunnel or invitation parameters.",
    action: "Submit the action again as a new request without modifying settings mid-flight.",
    actionLabel: "Retry Clean Request",
  },
  rate_limited: {
    code: "rate_limited",
    category: "api",
    httpStatus: 429,
    title: "Too many requests in a short time",
    cause: "You or a guest sent requests faster than the rate limit allows.",
    action: "Wait a moment for the cooldown timer (Retry-After) to finish, then try again.",
    actionLabel: "Wait & Retry",
  },
  quota_exceeded: {
    code: "quota_exceeded",
    category: "api",
    httpStatus: 403,
    title: "You've reached your active tunnel or data limit",
    cause: "Your account has reached its maximum concurrent tunnels or monthly relay egress allowance.",
    action: "Stop an existing active tunnel or wait for your quota cycle to reset before starting another.",
    actionLabel: "Manage Active Tunnels",
  },
  invitation_invalid: {
    code: "invitation_invalid",
    category: "api",
    httpStatus: 400,
    title: "This invitation isn't valid",
    cause: "The invitation link has expired, was revoked by the host, reached its maximum claims, or was mistyped.",
    action: "Ask the host to generate and share a fresh invitation link.",
    actionLabel: "Create New Invitation",
  },
  relay_unavailable: {
    code: "relay_unavailable",
    category: "api",
    httpStatus: 503,
    title: "We can't reach a connection server right now",
    cause: "No healthy relay server with available port capacity is reachable in your region.",
    action: "Click Retry to request another relay allocation, or check service status if the outage persists.",
    actionLabel: "Retry Connection",
  },
  client_upgrade_required: {
    code: "client_upgrade_required",
    category: "api",
    httpStatus: 426,
    title: "App update required to connect",
    cause: "This version of the desktop app is older than the minimum protocol version supported by the relay.",
    action: "Download and verify the latest signed release, then restart the app.",
    actionLabel: "Check for Updates",
  },
  internal: {
    code: "internal",
    category: "api",
    httpStatus: 500,
    title: "Something went wrong on our side",
    cause: "The control plane encountered an unexpected server error while processing your request.",
    action: "Try the action again in a few seconds. If it continues, export a redacted diagnostics bundle for support.",
    actionLabel: "Export Diagnostics",
  },

  // 7 Engine & Network Failure Classes (FR-015, user-flows.md §§11–12)
  udp_blocked: {
    code: "udp_blocked",
    category: "engine",
    httpStatus: null,
    title: "Your network is blocking outbound UDP traffic",
    cause: "A local firewall, campus network, or router blocked the outbound QUIC/UDP connection to the relay.",
    action: "Allow outbound UDP port 4433 in Windows Defender Firewall, or switch to a less restrictive network.",
    actionLabel: "Run Troubleshooter",
  },
  local_server_down: {
    code: "local_server_down",
    category: "engine",
    httpStatus: null,
    title: "Nothing is answering at your local server port",
    cause: "The L0 probe could not connect to your configured local game server address and port.",
    action: "Start your dedicated game server first, verify its port matches Configure Server, and click Check Again.",
    actionLabel: "Check Server Again",
  },
  local_server_wrong_bind: {
    code: "local_server_wrong_bind",
    category: "engine",
    httpStatus: null,
    title: "Game server may be listening on a different address",
    cause: "Loopback (127.0.0.1) refused the connection because the game server is bound to a specific LAN interface.",
    action: "Set server-ip to blank/127.0.0.1 in your game server config, or update Local Host Address and confirm.",
    actionLabel: "Edit Local Target",
  },
  optional_port_disabled: {
    code: "optional_port_disabled",
    category: "engine",
    httpStatus: null,
    title: "Optional Query or RCON port is off by default",
    cause: "Optional administrative and status ports (RCON, Query, SourceTV) stay disabled by default for security (GAME-008).",
    action: "Open Configure Server and explicitly enable the optional mapping only if you intend to expose it.",
    actionLabel: "Configure Mappings",
  },
  token_expired: {
    code: "token_expired",
    category: "engine",
    httpStatus: null,
    title: "Relay allocation token expired",
    cause: "The short-lived allocation token expired while your computer was asleep or disconnected from the network.",
    action: "Click Start Tunnel to request a fresh allocation token and reconnect.",
    actionLabel: "Reconnect Tunnel",
  },
  handshake_failed: {
    code: "handshake_failed",
    category: "engine",
    httpStatus: null,
    title: "Encrypted relay handshake timed out",
    cause: "The TLS 1.3 / QUIC handshake with the relay did not complete within the connection timeout.",
    action: "Check your internet connection and system clock accuracy, then click Retry.",
    actionLabel: "Retry Handshake",
  },
  grace_period_expired: {
    code: "grace_period_expired",
    category: "engine",
    httpStatus: null,
    title: "Network interruption exceeded the reconnect window",
    cause: "Your connection was offline longer than the 30-second relay grace period, so the public port reservation was released.",
    action: "Start the tunnel again and share the new public address with your guests if the port changed.",
    actionLabel: "Start Tunnel Again",
  },

  // 6 Managed Server Engine Failure Codes (T-088, SRV-001, SRV-002, SRV-006, user-flows.md §5 step 6)
  srv_download_failed: {
    code: "srv_download_failed",
    category: "engine",
    httpStatus: null,
    title: "Managed server download could not complete",
    cause: "The official server archive or Java runtime download was interrupted before completion.",
    action: "Check your internet connection and click Retry Provisioning to resume from a clean download.",
    actionLabel: "Retry Provisioning",
  },
  srv_verification_failed: {
    code: "srv_verification_failed",
    category: "engine",
    httpStatus: null,
    title: "Downloaded server file failed SHA-256 verification",
    cause: "The downloaded artifact's cryptographic hash did not match the pinned official manifest. The unverified file was discarded and was never executed.",
    action: "Do not run unverified binaries. Click Retry Provisioning to fetch a fresh copy from the official distribution mirror.",
    actionLabel: "Discard & Retry",
  },
  srv_terms_not_accepted: {
    code: "srv_terms_not_accepted",
    category: "engine",
    httpStatus: null,
    title: "Game server EULA confirmation is required",
    cause: "The app cannot accept the Minecraft End User License Agreement on your behalf (SRV-001).",
    action: "Review the official EULA link, check the host confirmation box, or return to manual configuration.",
    actionLabel: "Confirm EULA Terms",
  },
  srv_disk_space_insufficient: {
    code: "srv_disk_space_insufficient",
    category: "engine",
    httpStatus: null,
    title: "Not enough free disk space for managed server",
    cause: "The local per-user server directory requires at least 500 MB of free disk space for the runtime, server jar, and world backups.",
    action: "Free up disk space on your local drive and click Retry Provisioning.",
    actionLabel: "Retry After Freeing Space",
  },
  srv_process_crashed: {
    code: "srv_process_crashed",
    category: "engine",
    httpStatus: null,
    title: "Managed game server exited unexpectedly",
    cause: "The local server process terminated with a non-zero exit code. Automatic bounded recovery is attempting a restart.",
    action: "Expand Details to inspect the server log excerpt, or restore the latest world backup if world data was corrupted.",
    actionLabel: "Inspect Crash Log",
  },
  srv_restart_limit_reached: {
    code: "srv_restart_limit_reached",
    category: "engine",
    httpStatus: null,
    title: "Managed server stopped after repeated crashes",
    cause: "The server crashed 3 times in a row, reaching the bounded automatic restart limit.",
    action: "Open Show Log to inspect the root cause, restore a known-good world backup, and click Start Server Again.",
    actionLabel: "Show Log & Start Again",
  },
};

function lookupErrorGuidance(codeOrError) {
  const code =
    typeof codeOrError === "string"
      ? codeOrError.trim().toLowerCase()
      : String(codeOrError?.error?.code || codeOrError?.code || "").trim().toLowerCase();
  return ERROR_MESSAGE_CATALOG[code] || ERROR_MESSAGE_CATALOG.internal;
}

// Guided 6-Step Troubleshooter (user-flows.md §12, FR-015, UX-003)
function evaluateTroubleshooterChecks() {
  const primaryMapping = state.selectedProfile.mappings[0];
  const primaryCfg = state.mappingConfigs[primaryMapping.key] || {
    host: "127.0.0.1",
    port: primaryMapping.defaultPort,
  };

  const step1Pass = Boolean(state.isSignedIn);
  const step2Pass = state.tunnelState !== "Failed";
  const step3Pass = state.tunnelState === "Connected";
  const step4Unknown = state.localServerState === "Unknown";
  const step4Pass = state.localServerState === "Reachable";
  const step5Pass = typeof state.playersSeenCount === "number" && state.playersSeenCount > 0;

  return [
    {
      step: 1,
      id: "signed_in",
      label: "Signed in and session token valid",
      status: step1Pass ? "pass" : "fail",
      summary: step1Pass
        ? "Authenticated via system browser (RFC 8252 PKCE S256)."
        : ERROR_MESSAGE_CATALOG.unauthenticated.cause,
      action: step1Pass ? "No action needed." : ERROR_MESSAGE_CATALOG.unauthenticated.action,
    },
    {
      step: 2,
      id: "relay_reachable",
      label: "Relay server reachable",
      status: step2Pass ? "pass" : "fail",
      summary: step2Pass
        ? `Relay ${SESSION_SUMMARY.relayRegion} reachable (${SESSION_SUMMARY.meanRttMs} ms QUIC RTT).`
        : ERROR_MESSAGE_CATALOG.relay_unavailable.cause,
      action: step2Pass ? "No action needed." : ERROR_MESSAGE_CATALOG.relay_unavailable.action,
    },
    {
      step: 3,
      id: "tunnel_active",
      label: "Tunnel session Active",
      status: step3Pass ? "pass" : state.tunnelState === "Degraded" || state.tunnelState === "Reconnecting" ? "warn" : "fail",
      summary: step3Pass
        ? `Tunnel is Connected and forwarding to ${state.publicAddress}.`
        : `Tunnel is currently ${state.tunnelState}.`,
      action: step3Pass
        ? "No action needed."
        : "Click Start Tunnel on the dashboard to establish the relay session.",
    },
    {
      step: 4,
      id: "local_server_reachable",
      label: `Local server reachable (${primaryCfg.host}:${primaryCfg.port})`,
      status: step4Pass ? "pass" : step4Unknown ? "unknown" : "fail",
      summary: step4Pass
        ? `L0 probe connected to ${primaryCfg.host}:${primaryCfg.port} in 1.1 ms.`
        : step4Unknown
        ? "Cannot be probed automatically for this UDP mapping; check the game server console manually."
        : `Nothing is answering at ${primaryCfg.host}:${primaryCfg.port}.`,
      action: step4Pass
        ? "No action needed."
        : step4Unknown
        ? "Verify the UDP game server window shows it is listening on the configured port."
        : ERROR_MESSAGE_CATALOG.local_server_down.action,
    },
    {
      step: 5,
      id: "guest_traffic_observed",
      label: "Guest traffic observed at relay",
      status: step5Pass ? "pass" : "warn",
      summary: step5Pass
        ? `${state.playersSeenCount} guest source flow(s) observed (${OBSERVED_TRAFFIC_LABEL}).`
        : "No guest packets have arrived through the relay yet.",
      action: step5Pass
        ? "Tunnel is carrying guest traffic."
        : "Verify the guest claimed a valid invitation link on the same network they use to play.",
    },
    {
      step: 6,
      id: "game_level_reminders",
      label: "Game-level settings check (whitelist, version, password)",
      status: "info",
      summary:
        "Tunnel status and observed traffic do not prove the game server accepted the player into the world.",
      action:
        "If traffic is observed in Step 5 but a player still cannot join, check game version parity, server password, allowlist/whitelist, and online-mode.",
    },
  ];
}

// Redacted diagnostics export (FR-013, PRIV-002, INV-5)
function exportRedactedDiagnostics() {
  const checks = evaluateTroubleshooterChecks();
  const bundle = {
    schema_version: 1,
    exported_at: "2026-11-04T14:25:00Z",
    profile_id: state.selectedProfile.id,
    status_layers: {
      tunnel_state: state.tunnelState,
      local_server_state: state.localServerState,
      players_seen_count: state.playersSeenCount,
      players_seen_label: OBSERVED_TRAFFIC_LABEL,
    },
    troubleshooter_steps: checks,
    active_guest_subnets_masked: ACTIVE_GUEST_FLOWS.map((f) => f.guestSubnet),
    redaction_policy: "tokens=[REDACTED_TOKEN] secrets=[REDACTED_SECRET] guest_ips=/24_masked payloads=excluded",
  };
  state.lastDiagnosticsExport = bundle;
  showToast("Redacted diagnostics bundle exported (0 secrets, /24 masked)");
  render();
  return bundle;
}

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
  const navInvBadge = document.getElementById("nav-badge-invitations");
  if (navInvBadge) {
    const activeInvCount = state.invitations.filter((i) => i.status === "active").length;
    navInvBadge.textContent = String(activeInvCount);
  }
  const navTroubleBadge = document.getElementById("nav-badge-troubleshooter");
  if (navTroubleBadge) {
    const checks = evaluateTroubleshooterChecks();
    const passCount = checks.filter((c) => c.status === "pass" || c.status === "info").length;
    navTroubleBadge.textContent = `${passCount}/${checks.length}`;
  }
  const navManagedBadge = document.getElementById("nav-badge-managed");
  if (navManagedBadge) {
    navManagedBadge.textContent = state.managedServer.state === "Running" ? "RUN" : "Java";
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
  } else if (state.currentView === "managed-server") {
    if (viewTitle) viewTitle.textContent = "Managed Server Setup (S12)";
    mainContent.innerHTML = renderManagedServerView();
    bindManagedServerEvents();
  } else if (state.currentView === "invitations") {
    if (viewTitle) viewTitle.textContent = "Invitations & Guest Sharing";
    mainContent.innerHTML = renderInvitationsView();
    bindInvitationsEvents();
  } else if (state.currentView === "troubleshooter") {
    if (viewTitle) viewTitle.textContent = "Troubleshooter & Error Catalog";
    mainContent.innerHTML = renderTroubleshooterView();
    bindTroubleshooterEvents();
  } else {
    if (viewTitle) viewTitle.textContent = "Tunnel Dashboard";
    mainContent.innerHTML = renderDashboardView();
    bindDashboardEvents();
  }

  updateSidebarNav();
  restoreDialogFocusIfNeeded();
}

function restoreDialogFocusIfNeeded() {
  if (state.isStopModalOpen) {
    state.activeFocusedElementId = "btn-cancel-stop";
    document.getElementById("btn-cancel-stop")?.focus?.();
  } else if (state.isManagedUpdateModalOpen) {
    state.activeFocusedElementId = "btn-cancel-managed-update";
    document.getElementById("btn-cancel-managed-update")?.focus?.();
  }
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

// S5: Local Server Configuration Screen (FR-003, GAME-009, GAME-008, SRV-003, contracts.md §13)
function renderLocalConfigView() {
  const profile = state.selectedProfile;
  const generalError = state.validationErrors._general;
  const provisionable = isProfileProvisionable(profile.id);

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
        ${
          provisionable
            ? `<button type="button" id="btn-setup-managed-server" class="gtt-btn gtt-btn-primary">
                Set this up for me
               </button>`
            : ""
        }
      </div>
    </div>

    <!-- SRV-003 & user-flows.md §5 Journey 5b: Provisioning Offer vs Plain Manual-Only Notice -->
    ${
      provisionable
        ? `
      <section id="provisionable-offer-banner" class="disclosure-callout" role="region" aria-label="Managed server setup option">
        <strong>Managed Server Provisioning Available (<code data-ui>SRV-001</code>, <code data-ui>SRV-003</code>):</strong>
        Click <strong>Set this up for me</strong> to download the official Minecraft: Java Edition server jar and pinned Java 21 runtime, verify SHA-256 digests before running, and generate safe loopback defaults (<code data-ui>127.0.0.1:25565</code>, RCON off, Query off). Manual configuration below remains equally supported.
      </section>
    `
        : `
      <section id="manual-only-provisioning-note" class="disclosure-callout" role="note" aria-label="Manual server configuration notice">
        <strong>Manual server configuration only for ${profile.name} (<code data-ui>SRV-003</code>):</strong>
        Automatic server provisioning is offered only for profiles on the verified provisionable list (<code data-ui>minecraft-java</code>). Start your ${profile.name} dedicated server manually and enter its local address and port below.
      </section>
    `
    }

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

  document.getElementById("btn-setup-managed-server")?.addEventListener("click", () => {
    navigateTo("managed-server");
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

// S7: Three-Layer Independent Status Machine (T-083, FR-010, FR-012, UX-002, UX-006, DESIGN.md §6)
const OBSERVED_TRAFFIC_LABEL = "Observed relay traffic (not confirmed in-game presence)";

const TUNNEL_STATE_SPEC = {
  "Not running": { token: "neutral", pill: "pill--idle", iconShape: "hollow-circle", ariaLive: "off" },
  Connecting: { token: "info", pill: "pill--info", iconShape: "spinner", ariaLive: "polite" },
  Connected: { token: "ok", pill: "pill--ok", iconShape: "check-circle", ariaLive: "polite" },
  Degraded: { token: "warn", pill: "pill--warn", iconShape: "triangle", ariaLive: "polite" },
  Reconnecting: { token: "warn", pill: "pill--warn", iconShape: "circular-arrows", ariaLive: "polite" },
  Failed: { token: "error", pill: "pill--err", iconShape: "octagon-cross", ariaLive: "assertive" },
};

const LOCAL_SERVER_STATE_SPEC = {
  Reachable: { token: "ok", pill: "pill--ok", iconShape: "check" },
  "Not reachable": { token: "error", pill: "pill--err", iconShape: "cross" },
  Checking: { token: "info", pill: "pill--info", iconShape: "spinner" },
  Unknown: { token: "neutral", pill: "pill--idle", iconShape: "question-mark" },
};

// Maps engine state machine states (contracts.md §12) to UI labels (DESIGN.md §6)
function mapEngineTunnelState(engineState) {
  const normalized = String(engineState || "").trim();
  if (normalized === "Idle" || normalized === "Stopped" || normalized === "Not running") {
    return "Not running";
  }
  if (normalized === "Connecting" || normalized === "Authenticating") {
    return "Connecting";
  }
  if (normalized === "Active" || normalized === "Connected") {
    return "Connected";
  }
  if (normalized === "Degraded") {
    return "Degraded";
  }
  if (normalized === "Reconnecting") {
    return "Reconnecting";
  }
  if (normalized === "Failed") {
    return "Failed";
  }
  return "Not running";
}

// 1. Update Tunnel indicator independently (never mutates localServerState or playersSeenCount)
function setTunnelState(nextState) {
  const mapped = TUNNEL_STATE_SPEC[nextState] ? nextState : mapEngineTunnelState(nextState);
  state.tunnelState = mapped;
  render();
  return state.tunnelState;
}

// 2. Update Local Server indicator independently (never mutates tunnelState or playersSeenCount)
function setLocalServerState(nextServerState) {
  if (!LOCAL_SERVER_STATE_SPEC[nextServerState]) {
    throw new Error(`Invalid local server state: ${nextServerState}`);
  }
  state.localServerState = nextServerState;
  render();
  return state.localServerState;
}

// 3. Update Players Seen indicator independently (never mutates tunnelState or localServerState)
function setPlayersSeen(countOrUnavailable) {
  if (countOrUnavailable === null || countOrUnavailable === "Not available") {
    state.playersSeenCount = null;
  } else {
    const n = Number(countOrUnavailable);
    if (!Number.isInteger(n) || n < 0) {
      throw new Error(`Invalid playersSeenCount: ${countOrUnavailable}`);
    }
    state.playersSeenCount = n;
  }
  render();
  return state.playersSeenCount;
}

function formatPlayersSeenText(count) {
  if (count === null || count === undefined) return "Not available";
  if (count === 0) return "No traffic seen";
  if (count === 1) return "1 guest seen";
  return `${count} guests seen`;
}

// Stop tunnel confirmation gate (UX-006): confirmation appears ONLY when playersSeenCount > 0
// Initial focus lands on the safest action (#btn-cancel-stop, "Keep Running") per DESIGN.md §7.
function requestStopTunnel() {
  if (typeof state.playersSeenCount === "number" && state.playersSeenCount > 0) {
    state.isStopModalOpen = true;
    state.activeFocusedElementId = "btn-cancel-stop";
    render();
    return { confirmedImmediately: false, modalOpened: true, initialFocusId: "btn-cancel-stop" };
  }
  state.isStopModalOpen = false;
  executeStopTunnel();
  return { confirmedImmediately: true, modalOpened: false, initialFocusId: null };
}

function cancelStopTunnel() {
  state.isStopModalOpen = false;
  state.activeFocusedElementId = "btn-stop-tunnel";
  render();
}

function confirmStopTunnel() {
  state.isStopModalOpen = false;
  state.activeFocusedElementId = "btn-start-tunnel";
  executeStopTunnel();
}

// ============================================================================
// T-088: Managed Server Provisioning & 4th Independent Indicator (SRV-001..SRV-011, DESIGN.md §6 & §8)
// ============================================================================
// SRV-003 & ADR-013: Provisioning is offered ONLY for profiles on the verified provisionable list.
const PROVISIONABLE_PROFILE_IDS = new Set(["minecraft-java"]);

function isProfileProvisionable(profileId) {
  return PROVISIONABLE_PROFILE_IDS.has(String(profileId || "").trim().toLowerCase());
}

const MANAGED_SERVER_STATE_SPEC = {
  "Not set up": { token: "neutral", pill: "pill--idle", iconShape: "hollow-circle", ariaLive: "off" },
  Downloading: { token: "info", pill: "pill--info", iconShape: "spinner", ariaLive: "polite" },
  Verifying: { token: "info", pill: "pill--info", iconShape: "spinner", ariaLive: "polite" },
  Configuring: { token: "info", pill: "pill--info", iconShape: "spinner", ariaLive: "polite" },
  Starting: { token: "info", pill: "pill--info", iconShape: "spinner", ariaLive: "polite" },
  Running: { token: "ok", pill: "pill--ok", iconShape: "check", ariaLive: "polite" },
  Crashed: { token: "error", pill: "pill--err", iconShape: "cross", ariaLive: "assertive" },
  Updating: { token: "info", pill: "pill--info", iconShape: "spinner", ariaLive: "polite" },
  Failed: { token: "error", pill: "pill--err", iconShape: "octagon-cross", ariaLive: "assertive" },
};

// Every managed server state has a distinct, reviewed message with a cause and an action (T-088 Done-when)
const MANAGED_SERVER_STATE_MESSAGES = {
  "Not set up": {
    state: "Not set up",
    headline: "Managed game server is not installed yet",
    cause: "No local managed server instance has been provisioned in your per-user app directory.",
    action: "Confirm the game EULA terms and click Provision & Start Server, or configure an existing server manually.",
  },
  Downloading: {
    state: "Downloading",
    headline: "Downloading official server archive and runtime",
    cause: "Fetching the pinned Minecraft: Java Edition server jar and Java 21 runtime over HTTPS.",
    action: "Wait for the download to finish, or click Cancel Provisioning to stop and remove partial files.",
  },
  Verifying: {
    state: "Verifying",
    headline: "Verifying cryptographic SHA-256 digest before execution",
    cause: "Checking the downloaded archive against the pinned SHA-256 digest so no unverified code is ever run (SRV-002).",
    action: "Wait for verification to complete, or click Cancel Provisioning to abort before configuration.",
  },
  Configuring: {
    state: "Configuring",
    headline: "Writing safe loopback server configuration",
    cause: "Generating server.properties bound to 127.0.0.1:25565 with RCON off, Query off, and online-mode on (SRV-004).",
    action: "Review the generated configuration summary below while setup completes.",
  },
  Starting: {
    state: "Starting",
    headline: "Starting local managed server process",
    cause: "Launching the verified Java server process inside a Windows Job Object with kill-on-close protection (SRV-007).",
    action: "Wait for the server to finish loading the world and bind to 127.0.0.1:25565.",
  },
  Running: {
    state: "Running",
    headline: "Managed server process is running on 127.0.0.1:25565",
    cause: "The managed Minecraft: Java Edition server process is active and bound to loopback.",
    action: "Create a world backup, check for updates, or stop the managed server when your session ends.",
  },
  Crashed: {
    state: "Crashed",
    headline: "Managed server process exited unexpectedly",
    cause: "The server process terminated with a non-zero exit code. Bounded automatic restart is tracking attempts.",
    action: "Expand Details to inspect the crash log excerpt, restore a world backup, or click Start Server Again.",
  },
  Updating: {
    state: "Updating",
    headline: "Applying verified server version update",
    cause: "Created an automatic pre-update world backup and verifying the new server version archive (SRV-009).",
    action: "Wait for the update verification and restart sequence to complete.",
  },
  Failed: {
    state: "Failed",
    headline: "Managed server setup or recovery could not complete",
    cause: "A provisioning check (terms, download, SHA-256 verification, disk space, or crash limit) stopped execution.",
    action: "Review the specific failure cause and recommended action below, then click Retry Provisioning.",
  },
};

// 4. Update Managed Server indicator independently (SRV-006: NEVER mutates tunnelState, localServerState, or playersSeenCount)
function setManagedServerState(nextState, options = {}) {
  if (!MANAGED_SERVER_STATE_SPEC[nextState]) {
    throw new Error(`Invalid managed server state: ${nextState}`);
  }
  const ms = state.managedServer;
  ms.state = nextState;

  if (options.progressPercent !== undefined) {
    ms.progressPercent = Number(options.progressPercent);
  }
  if (options.failureCode !== undefined) {
    ms.failureCode = options.failureCode;
  } else if (nextState !== "Failed" && nextState !== "Crashed") {
    ms.failureCode = null;
  }
  if (options.restartAttempt !== undefined) {
    ms.restartAttempt = Number(options.restartAttempt);
  }

  // Update step statuses to match discrete state
  if (nextState === "Not set up") {
    ms.activeStep = null;
    ms.progressPercent = 0;
    ms.stepStatuses = { download: "pending", verify: "pending", configure: "pending", start: "pending" };
  } else if (nextState === "Downloading") {
    ms.activeStep = "download";
    if (!options.progressPercent) ms.progressPercent = 25;
    ms.stepStatuses = { download: "active", verify: "pending", configure: "pending", start: "pending" };
  } else if (nextState === "Verifying") {
    ms.activeStep = "verify";
    if (!options.progressPercent) ms.progressPercent = 55;
    ms.stepStatuses = { download: "completed", verify: "active", configure: "pending", start: "pending" };
  } else if (nextState === "Configuring") {
    ms.activeStep = "configure";
    if (!options.progressPercent) ms.progressPercent = 80;
    ms.artifactVerified = true;
    ms.stepStatuses = { download: "completed", verify: "completed", configure: "active", start: "pending" };
  } else if (nextState === "Starting") {
    ms.activeStep = "start";
    if (!options.progressPercent) ms.progressPercent = 92;
    ms.artifactVerified = true;
    ms.stepStatuses = { download: "completed", verify: "completed", configure: "completed", start: "active" };
  } else if (nextState === "Running") {
    ms.activeStep = null;
    ms.progressPercent = 100;
    ms.artifactVerified = true;
    ms.artifactDiscarded = false;
    ms.cancelSummary = null;
    ms.stepStatuses = { download: "completed", verify: "completed", configure: "completed", start: "completed" };
  } else if (nextState === "Crashed") {
    if (!ms.failureCode) ms.failureCode = "srv_process_crashed";
    if (!ms.restartAttempt) ms.restartAttempt = 1;
  } else if (nextState === "Failed") {
    if (!ms.failureCode) ms.failureCode = "srv_download_failed";
    if (ms.failureCode === "srv_verification_failed") {
      // SRV-002 & user-flows.md §5 step 6: Verification failure discards the artifact and never runs it
      ms.artifactVerified = false;
      ms.artifactDiscarded = true;
      ms.stepStatuses = { download: "completed", verify: "failed", configure: "pending", start: "pending" };
    }
  }

  render();
  return ms.state;
}

function acceptManagedServerTerms(accepted) {
  state.managedServer.termsAccepted = Boolean(accepted);
  if (state.managedServer.termsAccepted && state.managedServer.failureCode === "srv_terms_not_accepted") {
    state.managedServer.failureCode = null;
    if (state.managedServer.state === "Failed") {
      state.managedServer.state = "Not set up";
    }
  }
  render();
  return state.managedServer.termsAccepted;
}

function startManagedServerProvisioning() {
  if (!isProfileProvisionable(state.selectedProfile.id)) {
    return {
      ok: false,
      error: {
        code: "validation_failed",
        message: `Profile ${state.selectedProfile.id} is not on the provisionable allow-list (SRV-003).`,
      },
    };
  }
  if (!state.managedServer.termsAccepted) {
    setManagedServerState("Failed", { failureCode: "srv_terms_not_accepted", progressPercent: 0 });
    return {
      ok: false,
      error: lookupErrorGuidance("srv_terms_not_accepted"),
    };
  }
  state.managedServer.cancelSummary = null;
  state.managedServer.artifactDiscarded = false;
  setManagedServerState("Downloading", { progressPercent: 25 });
  return { ok: true, state: state.managedServer.state };
}

function advanceManagedServerProvisioningStep() {
  const current = state.managedServer.state;
  if (current === "Downloading") {
    setManagedServerState("Verifying", { progressPercent: 55 });
  } else if (current === "Verifying") {
    setManagedServerState("Configuring", { progressPercent: 80 });
  } else if (current === "Configuring") {
    setManagedServerState("Starting", { progressPercent: 92 });
  } else if (current === "Starting") {
    setManagedServerState("Running", { progressPercent: 100 });
  }
  return state.managedServer.state;
}

function cancelManagedServerProvisioning() {
  const previousState = state.managedServer.state;
  const previousProgress = state.managedServer.progressPercent;
  state.managedServer.cancelSummary = `Cancelled during ${previousState} (${previousProgress}%). Temporary unverified files were removed; no server process was started.`;
  setManagedServerState("Not set up", { progressPercent: 0 });
  return state.managedServer.cancelSummary;
}

function stopManagedServer() {
  state.managedServer.confirmedInGameCount = null;
  setManagedServerState("Not set up", { progressPercent: 0 });
  showToast("Managed server stopped cleanly");
  return state.managedServer.state;
}

let backupSeqCounter = 200;
function createManagedWorldBackup(label = "Manual world backup") {
  backupSeqCounter += 1;
  const bkp = {
    id: `bkp_01J9K9_${backupSeqCounter}`,
    label: String(label || "Manual world backup").trim(),
    version: state.managedServer.currentVersion,
    createdAt: "14:26:00 UTC",
    sizeMb: 49.2,
  };
  state.managedServer.backups.unshift(bkp);
  showToast(`World backup created (${bkp.id})`);
  render();
  return bkp;
}

function restoreManagedWorldBackup(backupId) {
  const found = state.managedServer.backups.find((b) => b.id === backupId);
  if (!found) {
    return { ok: false, error: { code: "not_found", message: "Backup snapshot not found." } };
  }
  state.managedServer.lastRestoredBackupId = found.id;
  state.managedServer.currentVersion = found.version;
  showToast(`Restored world backup ${found.id}`);
  render();
  return { ok: true, restoredBackup: { ...found } };
}

// SRV-009 & DESIGN.md §7: Applying a game server update while a tunnel is active requires confirmation
function requestManagedServerUpdate(targetVersion = state.managedServer.availableUpdateVersion) {
  const isTunnelActive =
    state.tunnelState === "Connected" ||
    state.tunnelState === "Degraded" ||
    state.tunnelState === "Reconnecting";

  state.pendingManagedUpdateVersion = targetVersion;
  if (isTunnelActive) {
    state.isManagedUpdateModalOpen = true;
    state.activeFocusedElementId = "btn-cancel-managed-update";
    render();
    return {
      updatedImmediately: false,
      modalOpened: true,
      initialFocusId: "btn-cancel-managed-update",
    };
  }

  state.isManagedUpdateModalOpen = false;
  executeManagedServerUpdate(targetVersion);
  return {
    updatedImmediately: true,
    modalOpened: false,
    initialFocusId: null,
  };
}

function cancelManagedServerUpdate() {
  state.isManagedUpdateModalOpen = false;
  state.pendingManagedUpdateVersion = null;
  state.activeFocusedElementId = "btn-update-managed-server";
  render();
}

function confirmManagedServerUpdate() {
  const targetVersion = state.pendingManagedUpdateVersion || state.managedServer.availableUpdateVersion;
  state.isManagedUpdateModalOpen = false;
  state.pendingManagedUpdateVersion = null;
  executeManagedServerUpdate(targetVersion);
}

function executeManagedServerUpdate(targetVersion) {
  // SRV-009: Create pre-update backup automatically before updating
  createManagedWorldBackup(`Pre-update backup (v${state.managedServer.currentVersion})`);
  setManagedServerState("Updating", { progressPercent: 65 });
  state.managedServer.currentVersion = targetVersion;
  setManagedServerState("Running", { progressPercent: 100 });
  showToast(`Managed server updated to v${targetVersion}`);
}

// SRV-011 & user-flows.md §0: Opt-in Server Management Protocol confirmed in-game presence
// Never merges with state.playersSeenCount ("N guests seen")
function setConfirmedInGamePresence(countOrNull) {
  if (countOrNull === null || countOrNull === undefined) {
    state.managedServer.serverManagementProtocolEnabled = false;
    state.managedServer.confirmedInGameCount = null;
  } else {
    const n = Number(countOrNull);
    if (!Number.isInteger(n) || n < 0) {
      throw new Error(`Invalid confirmedInGameCount: ${countOrNull}`);
    }
    state.managedServer.serverManagementProtocolEnabled = true;
    state.managedServer.confirmedInGameCount = n;
  }
  render();
  return state.managedServer.confirmedInGameCount;
}

// S7: Dashboard View with Divided 4-Cell Status Strip, SVG Telemetry Chart & Reconciled Flows Table
function renderDashboardView() {
  const isTunnelActive =
    state.tunnelState === "Connected" ||
    state.tunnelState === "Degraded" ||
    state.tunnelState === "Reconnecting";
  const isTunnelConnecting = state.tunnelState === "Connecting";

  const tunnelSpec = TUNNEL_STATE_SPEC[state.tunnelState] || TUNNEL_STATE_SPEC["Not running"];
  const tunnelBadgeClass = tunnelSpec.token;

  const serverSpec =
    LOCAL_SERVER_STATE_SPEC[state.localServerState] || LOCAL_SERVER_STATE_SPEC["Unknown"];
  const serverBadgeClass = serverSpec.token;

  const hasObservedPlayers = typeof state.playersSeenCount === "number" && state.playersSeenCount > 0;
  const playersBadgeClass = hasObservedPlayers ? "ok" : "neutral";
  const playersLabelText = formatPlayersSeenText(state.playersSeenCount);

  const managedSpec =
    MANAGED_SERVER_STATE_SPEC[state.managedServer.state] || MANAGED_SERVER_STATE_SPEC["Not set up"];
  const managedBadgeClass = managedSpec.token;
  const confirmedCount = state.managedServer.confirmedInGameCount;
  const confirmedPresenceText =
    state.managedServer.serverManagementProtocolEnabled && typeof confirmedCount === "number"
      ? `${confirmedCount} in game (reported by the server)`
      : "Server Management Protocol off · traffic count only";

  const primaryMapping = state.selectedProfile.mappings[0];
  const primaryCfg = state.mappingConfigs[primaryMapping.key] || {
    host: "127.0.0.1",
    port: primaryMapping.defaultPort,
  };

  const query = (state.filterQuery || "").trim().toLowerCase();
  const activeFlows = isTunnelActive
    ? ACTIVE_GUEST_FLOWS.slice(0, Math.max(0, Number(state.playersSeenCount) || 0)).filter(
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
      <div class="kpi" id="indicator-tunnel" data-indicator="tunnel" data-state="${state.tunnelState}" data-token="${tunnelBadgeClass}" aria-live="${tunnelSpec.ariaLive}">
        <div class="kpi__label">
          <span>Tunnel Connection</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${tunnelBadgeClass}">
            <span class="status-icon ${tunnelBadgeClass}" data-icon-shape="${tunnelSpec.iconShape}"></span>
            <span class="kpi__value" data-role="indicator-value">${state.tunnelState}</span>
          </div>
        </div>
        <div class="kpi__sub">QUIC v1 · ${SESSION_SUMMARY.meanRttMs} ms RTT to fra1</div>
      </div>

      <!-- 2. Local Server -->
      <div class="kpi" id="indicator-local-server" data-indicator="local-server" data-state="${state.localServerState}" data-token="${serverBadgeClass}" aria-live="polite">
        <div class="kpi__label">
          <span>Local Server (${primaryCfg.host})</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${serverBadgeClass}">
            <span class="status-icon ${serverBadgeClass}" data-icon-shape="${serverSpec.iconShape}"></span>
            <span class="kpi__value" data-role="indicator-value">${state.localServerState}</span>
          </div>
        </div>
        <div class="kpi__sub">${primaryMapping.protocol.toUpperCase()} port ${primaryCfg.port} · L0 connect probe</div>
      </div>

      <!-- 3. Guest Traffic Observed -->
      <div class="kpi" id="indicator-players-seen" data-indicator="players-seen" data-state="${playersLabelText}" data-token="${playersBadgeClass}" aria-live="polite">
        <div class="kpi__label">
          <span>Guest Traffic Observed</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${playersBadgeClass}">
            <span class="status-icon ${playersBadgeClass}" data-icon-shape="${hasObservedPlayers ? "check-circle" : "hollow-circle"}"></span>
            <span class="kpi__value" data-role="indicator-value">${playersLabelText}</span>
          </div>
        </div>
        <div class="kpi__sub" data-role="observed-traffic-disclosure">${OBSERVED_TRAFFIC_LABEL}</div>
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

    <!-- 4th Separate Status Indicator: Managed Server & Separate Confirmed Presence (SRV-006, SRV-011, DESIGN.md §6) -->
    <section class="managed-status-bar" role="region" aria-label="Managed server and confirmed in-game presence">
      <div class="kpi" id="indicator-managed-server" data-indicator="managed-server" data-state="${state.managedServer.state}" data-token="${managedBadgeClass}" aria-live="${managedSpec.ariaLive}">
        <div class="kpi__label">
          <span>Managed Server (4th Indicator · <code data-ui>SRV-006</code>)</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${managedBadgeClass}">
            <span class="status-icon ${managedBadgeClass}" data-icon-shape="${managedSpec.iconShape}"></span>
            <span class="kpi__value" data-role="indicator-value">${state.managedServer.state}</span>
          </div>
        </div>
        <div class="kpi__sub">${MANAGED_SERVER_STATE_MESSAGES[state.managedServer.state]?.headline || ""}</div>
      </div>

      <div class="kpi" id="indicator-confirmed-presence" data-indicator="confirmed-presence" data-state="${confirmedPresenceText}" data-token="${typeof confirmedCount === "number" ? "ok" : "neutral"}" aria-live="polite">
        <div class="kpi__label">
          <span>Confirmed In-Game Presence (Opt-in · <code data-ui>SRV-011</code>)</span>
        </div>
        <div class="kpi__value-row">
          <span class="kpi__value" style="font-size: 16px;">${confirmedPresenceText}</span>
        </div>
        <div class="kpi__sub">Separate from Guest Traffic Observed (<code data-ui>UX-002</code>)</div>
      </div>

      <div style="display: flex; justify-content: flex-end;">
        <button type="button" id="btn-open-managed-server-dash" class="gtt-btn gtt-btn-secondary gtt-btn-sm">
          Open Managed Server Setup
        </button>
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

  document.getElementById("btn-open-managed-server-dash")?.addEventListener("click", () => {
    navigateTo("managed-server");
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
    requestStopTunnel();
  });

  document.getElementById("btn-cancel-stop")?.addEventListener("click", () => {
    cancelStopTunnel();
  });

  document.getElementById("btn-confirm-stop")?.addEventListener("click", () => {
    confirmStopTunnel();
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

// S8 & G1: Invitations, One-Time Secret Banner, Revocation & Guest Claim View (T-084, FR-007, FR-008, FR-009, SEC-007)
function renderInvitationsView() {
  const banner = state.oneTimeInvitationBanner;
  const invitations = listInvitations();
  const claimPreview = state.guestClaimPreviewResult;

  return `
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">Invitations &amp; Guest Admission Control</h2>
        <p class="sheet-subtitle">
          Create scoped, expiring invitation links (<code data-ui>FR-007</code>). Secrets have &ge;128 bits of entropy, are shown once at creation, and are never stored in plaintext (<code data-ui>SEC-007</code>).
        </p>
      </div>
      <div class="sheet-header__actions">
        <button type="button" id="btn-back-to-dash-from-inv" class="gtt-btn gtt-btn-secondary">
          Back to Dashboard
        </button>
      </div>
    </div>

    <!-- One-Time Secret Banner (Shown ONLY immediately after creation; FR-007, contracts.md §5.11) -->
    ${
      banner
        ? `
      <section id="one-time-secret-banner" class="panel" style="border-color: var(--accent); background-color: var(--subtle);" role="region" aria-label="One-time invitation secret">
        <div class="panel__head">
          <div>
            <h3 class="panel__title">One-Time Invitation Secret Created (<code data-ui>${banner.invitationId}</code>)</h3>
            <span class="panel__sub">Copy this secret now. For security, it will never be shown again once dismissed (<code data-ui>contracts.md §5.12</code>).</span>
          </div>
          <span class="pill pill--warn">Shown Once Only</span>
        </div>
        <div class="copy-field-container">
          <input type="text"
                 id="one-time-secret-input"
                 class="copy-input"
                 readonly
                 value="${banner.secret}"
                 aria-label="One-time invitation secret" />
          <button type="button" id="btn-copy-secret" class="gtt-btn gtt-btn-primary">
            Copy Secret
          </button>
          <button type="button" id="btn-dismiss-secret" class="gtt-btn gtt-btn-secondary">
            I've Saved It · Dismiss
          </button>
        </div>
      </section>
    `
        : ""
    }

    <div class="workspace-grid">
      <!-- Left 5 Columns: Create Invitation Form & Guest Claim Page Simulator (G1) -->
      <div class="col-5" style="display: flex; flex-direction: column; gap: 16px;">
        <form id="create-invitation-form" class="panel" novalidate>
          <div class="panel__head">
            <h3 class="panel__title">Create New Invitation (S8)</h3>
            <span class="pill pill--info">POST /v1/tunnels/{id}/invitations</span>
          </div>
          <div class="mapping-fields-grid">
            <div class="field-group">
              <label class="field-label" for="inv-expiry-select">Expires In</label>
              <select id="inv-expiry-select" class="field-input">
                <option value="1800">30 minutes (1800s)</option>
                <option value="3600" selected>1 hour (3600s)</option>
                <option value="14400">4 hours (14400s)</option>
                <option value="86400">24 hours (86400s)</option>
              </select>
            </div>
            <div class="field-group">
              <label class="field-label" for="inv-max-claims">Max Claims</label>
              <input type="number" id="inv-max-claims" class="field-input" value="4" min="1" max="32" />
            </div>
          </div>
          <button type="submit" id="btn-create-invitation" class="gtt-btn gtt-btn-primary" style="width: 100%;">
            Generate One-Time Invitation
          </button>
        </form>

        <!-- Guest Claim Page Simulator (G1, FR-008, user-flows.md §7) -->
        <form id="guest-claim-simulator-form" class="panel" novalidate>
          <div class="panel__head">
            <div>
              <h3 class="panel__title">Guest Claim Page Preview (G1)</h3>
              <span class="panel__sub">Simulates <code data-ui>POST /v1/invitations/claim</code></span>
            </div>
            <span class="pill pill--idle">Guest Web View</span>
          </div>
          <div class="field-group">
            <label class="field-label" for="sim-claim-secret">Invitation Secret to Claim</label>
            <div class="copy-field-container">
              <input type="text"
                     id="sim-claim-secret"
                     class="field-input"
                     style="flex: 1;"
                     value="${banner ? banner.secret : ""}"
                     placeholder="Paste gtt_inv_... secret" />
              <button type="submit" id="btn-simulate-claim" class="gtt-btn gtt-btn-secondary">
                Claim as Guest
              </button>
            </div>
          </div>

          ${
            claimPreview
              ? claimPreview.ok
                ? `
              <div id="guest-claim-preview-box" class="disclosure-callout" role="status">
                <strong>Admitted (${claimPreview.data.game_name}):</strong><br>
                Connect to <code data-ui>${claimPreview.data.public_address}</code> (admitted ${claimPreview.data.admitted_until}).<br>
                <span>${claimPreview.data.instruction}</span><br>
                <span style="font-size: 12px;">Tip: ${claimPreview.data.network_mismatch_tip}</span>
              </div>
            `
                : `
              <div id="guest-claim-preview-box" class="non-loopback-warning" role="alert">
                <strong>${claimPreview.error.message}</strong>
                <span style="font-size: 12px; color: var(--muted);">Code: <code data-ui>${claimPreview.error.code}</code> (uniform response for expired, revoked, exhausted, or unknown secrets)</span>
              </div>
            `
              : ""
          }
        </form>
      </div>

      <!-- Right 7 Columns: Invitations List (Never Shows Secrets) & Active Guest Admissions -->
      <div class="col-7" style="display: flex; flex-direction: column; gap: 16px;">
        <div class="panel panel--flush">
          <div class="panel__head">
            <div>
              <h3 class="panel__title">Tunnel Invitations (<code data-ui>FR-007</code>, <code data-ui>FR-009</code>)</h3>
              <span class="panel__sub">List responses never include invitation secrets (<code data-ui>contracts.md §5.12</code>)</span>
            </div>
            <span class="pill pill--ok">${invitations.filter((i) => i.status === "active").length} Active</span>
          </div>
          <div class="table-scroll">
            <table class="data-table" id="invitations-table">
              <thead>
                <tr>
                  <th>Invitation ID</th>
                  <th>Expires</th>
                  <th class="num">Claims Used</th>
                  <th class="num">Status</th>
                  <th class="num">Action</th>
                </tr>
              </thead>
              <tbody>
                ${invitations
                  .map((inv) => {
                    let pillClass = "pill--idle";
                    if (inv.status === "active") pillClass = "pill--ok";
                    if (inv.status === "revoked") pillClass = "pill--err";
                    if (inv.status === "exhausted") pillClass = "pill--warn";

                    return `
                      <tr data-invitation-id="${inv.id}" data-status="${inv.status}">
                        <td><code data-ui>${inv.id}</code></td>
                        <td>${inv.expiresAt}</td>
                        <td class="num">${inv.claimCount} / ${inv.maxClaims}</td>
                        <td class="num"><span class="pill ${pillClass}">${inv.status}</span></td>
                        <td class="num">
                          ${
                            inv.status === "active"
                              ? `<button type="button" class="gtt-btn gtt-btn-destructive gtt-btn-sm btn-revoke-invitation" data-invitation-id="${inv.id}">Revoke</button>`
                              : `<span style="color: var(--muted); font-size: 12px;">—</span>`
                          }
                        </td>
                      </tr>
                    `;
                  })
                  .join("")}
              </tbody>
            </table>
          </div>
        </div>

        <div class="panel panel--flush">
          <div class="panel__head">
            <div>
              <h3 class="panel__title">Guest Source Admissions (<code data-ui>/24</code> Masked)</h3>
              <span class="panel__sub">Revoking an invitation automatically revokes its linked admissions</span>
            </div>
          </div>
          <div class="table-scroll">
            <table class="data-table" id="admissions-table">
              <thead>
                <tr>
                  <th>Admission ID</th>
                  <th>Invitation</th>
                  <th>Masked Subnet</th>
                  <th class="num">Status</th>
                  <th class="num">Action</th>
                </tr>
              </thead>
              <tbody>
                ${state.guestAdmissions
                  .map(
                    (adm) => `
                  <tr data-admission-id="${adm.id}" data-status="${adm.status}">
                    <td><code data-ui>${adm.id}</code></td>
                    <td><code data-ui>${adm.invitationId}</code></td>
                    <td><code data-ui>${adm.guestSubnet}</code></td>
                    <td class="num">
                      <span class="pill ${adm.status === "active" ? "pill--ok" : "pill--err"}">${adm.status}</span>
                    </td>
                    <td class="num">
                      ${
                        adm.status === "active"
                          ? `<button type="button" class="gtt-btn gtt-btn-destructive gtt-btn-sm btn-revoke-admission" data-admission-id="${adm.id}">Revoke</button>`
                          : `<span style="color: var(--muted); font-size: 12px;">Revoked</span>`
                      }
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
    </div>
  `;
}

function bindInvitationsEvents() {
  document.getElementById("btn-back-to-dash-from-inv")?.addEventListener("click", () => {
    navigateTo("dashboard");
  });

  document.getElementById("btn-copy-secret")?.addEventListener("click", () => {
    const input = document.getElementById("one-time-secret-input");
    if (input) {
      navigator.clipboard?.writeText(input.value);
      showToast("One-time invitation secret copied to clipboard");
    }
  });

  document.getElementById("btn-dismiss-secret")?.addEventListener("click", () => {
    dismissOneTimeSecret();
  });

  document.getElementById("create-invitation-form")?.addEventListener("submit", (e) => {
    e.preventDefault();
    const expSelect = document.getElementById("inv-expiry-select");
    const maxInput = document.getElementById("inv-max-claims");
    const res = createInvitation({
      expiresInSeconds: expSelect ? Number(expSelect.value) : 3600,
      maxClaims: maxInput ? Number(maxInput.value) : 4,
    });
    if (res.ok) {
      showToast("Invitation created — copy your one-time secret now");
    } else {
      showToast(res.error.message);
    }
  });

  document.getElementById("guest-claim-simulator-form")?.addEventListener("submit", (e) => {
    e.preventDefault();
    const secretInput = document.getElementById("sim-claim-secret");
    const secretVal = secretInput ? secretInput.value : "";
    claimInvitation(secretVal, "198.51.100.0/24");
  });

  document.querySelectorAll(".btn-revoke-invitation").forEach((btn) => {
    btn.addEventListener("click", () => {
      const invId = btn.getAttribute("data-invitation-id");
      if (invId) {
        revokeInvitation(invId);
        showToast("Invitation revoked; may take a moment to take effect at relay");
      }
    });
  });

  document.querySelectorAll(".btn-revoke-admission").forEach((btn) => {
    btn.addEventListener("click", () => {
      const admId = btn.getAttribute("data-admission-id");
      if (admId) {
        revokeAdmission(admId);
        showToast("Guest admission revoked");
      }
    });
  });
}

// S9: Guided Troubleshooter & Reviewed Error Message Catalog View (T-085, FR-015, UX-003, contracts.md §3)
function renderTroubleshooterView() {
  const checks = evaluateTroubleshooterChecks();
  const activeGuidance = lookupErrorGuidance(state.selectedErrorCode);
  const allCatalogEntries = Object.values(ERROR_MESSAGE_CATALOG);

  return `
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">Guided Troubleshooter &amp; Error Message Catalog</h2>
        <p class="sheet-subtitle">
          Ordered 6-step diagnostic checks (<code data-ui>user-flows.md §12</code>) and reviewed cause-and-action guidance for every error code in <code data-ui>contracts.md §3</code> (<code data-ui>FR-015</code>, <code data-ui>UX-003</code>).
        </p>
      </div>
      <div class="sheet-header__actions">
        <button type="button" id="btn-export-diagnostics" class="gtt-btn gtt-btn-secondary">
          Export Redacted Diagnostics (FR-013)
        </button>
        <button type="button" id="btn-rerun-troubleshooter" class="gtt-btn gtt-btn-primary">
          Re-Run Checks
        </button>
      </div>
    </div>

    <div class="workspace-grid">
      <!-- Left 6 Columns: 6-Step Ordered Troubleshooter (user-flows.md §12) -->
      <div class="panel panel--flush col-6">
        <div class="panel__head">
          <div>
            <h3 class="panel__title">6-Step Connection Troubleshooter (S9)</h3>
            <span class="panel__sub">Reports each layer independently; never claims game acceptance from traffic alone</span>
          </div>
        </div>
        <div class="table-scroll">
          <table class="data-table" id="troubleshooter-steps-table">
            <thead>
              <tr>
                <th>Step</th>
                <th>Diagnostic Check &amp; Next Action</th>
                <th class="num">Result</th>
              </tr>
            </thead>
            <tbody>
              ${checks
                .map((c) => {
                  let pillClass = "pill--idle";
                  if (c.status === "pass") pillClass = "pill--ok";
                  if (c.status === "warn") pillClass = "pill--warn";
                  if (c.status === "fail") pillClass = "pill--err";
                  if (c.status === "info") pillClass = "pill--info";

                  return `
                    <tr data-step-id="${c.id}" data-status="${c.status}">
                      <td><code data-ui>#${c.step}</code></td>
                      <td>
                        <div style="font-weight: 600; color: var(--text);">${c.label}</div>
                        <div style="font-size: 13px; color: var(--muted);">${c.summary}</div>
                        <div style="font-size: 12px; color: var(--text); margin-top: 2px;"><strong>Next step:</strong> ${c.action}</div>
                      </td>
                      <td class="num"><span class="pill ${pillClass}">${c.status.toUpperCase()}</span></td>
                    </tr>
                  `;
                })
                .join("")}
            </tbody>
          </table>
        </div>
      </div>

      <!-- Right 6 Columns: Active Error Inspector & Failure Class Simulator -->
      <div class="panel col-6">
        <div class="panel__head">
          <div>
            <h3 class="panel__title">Error Guidance Inspector (<code data-ui>UX-003</code>)</h3>
            <span class="panel__sub">Every error states a clear cause and a concrete next action</span>
          </div>
          <span class="pill ${activeGuidance.category === "api" ? "pill--info" : "pill--warn"}">
            ${activeGuidance.category.toUpperCase()} ${activeGuidance.httpStatus ? `· HTTP ${activeGuidance.httpStatus}` : "· Engine"}
          </span>
        </div>

        <div class="field-group">
          <label class="field-label" for="select-error-code">Inspect Error Code or Failure Class</label>
          <select id="select-error-code" class="field-input">
            ${allCatalogEntries
              .map(
                (entry) => `
              <option value="${entry.code}" ${entry.code === state.selectedErrorCode ? "selected" : ""}>
                ${entry.code} (${entry.category === "api" ? `HTTP ${entry.httpStatus}` : "Engine"}) · ${entry.title}
              </option>
            `
              )
              .join("")}
          </select>
        </div>

        <div id="error-inspector-card" class="disclosure-callout" role="region" aria-label="Reviewed error guidance">
          <div style="font-size: 15px; font-weight: 600; color: var(--text); margin-bottom: 6px;">
            ${activeGuidance.title} (<code data-ui>${activeGuidance.code}</code>)
          </div>
          <p style="margin-bottom: 6px;"><strong>Cause:</strong> ${activeGuidance.cause}</p>
          <p><strong>Next step:</strong> ${activeGuidance.action}</p>
        </div>
      </div>

      <!-- Full-Width 12 Columns: Complete Reviewed Message Catalog (contracts.md §3 + FR-015) -->
      <div class="panel panel--flush col-12">
        <div class="panel__head">
          <div>
            <h3 class="panel__title">Complete Error Message Catalog (<code data-ui>contracts.md §3</code> &amp; <code data-ui>FR-015</code>)</h3>
            <span class="panel__sub">12 API error codes + 7 engine failure classes · Zero secrets or internal IDs echoed</span>
          </div>
          <span class="pill pill--ok">${allCatalogEntries.length} Reviewed Codes</span>
        </div>
        <div class="table-scroll">
          <table class="data-table" id="error-catalog-table">
            <thead>
              <tr>
                <th>Code</th>
                <th>Source</th>
                <th>User-Facing Headline</th>
                <th>Cause Stated</th>
                <th>Next Action Stated</th>
              </tr>
            </thead>
            <tbody>
              ${allCatalogEntries
                .map(
                  (e) => `
                <tr data-error-code="${e.code}">
                  <td><code data-ui>${e.code}</code></td>
                  <td>${e.category === "api" ? `API (${e.httpStatus})` : "Engine"}</td>
                  <td style="font-weight: 600;">${e.title}</td>
                  <td>${e.cause}</td>
                  <td>${e.action}</td>
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

function bindTroubleshooterEvents() {
  document.getElementById("btn-rerun-troubleshooter")?.addEventListener("click", () => {
    showToast("Diagnostic checks completed");
    render();
  });

  document.getElementById("btn-export-diagnostics")?.addEventListener("click", () => {
    exportRedactedDiagnostics();
  });

  document.getElementById("select-error-code")?.addEventListener("change", (e) => {
    state.selectedErrorCode = e.target.value;
    render();
  });
}

// ============================================================================
// S12: Managed Server Setup, Provisioning, World Backup/Restore & Update View (T-088, SRV-001..SRV-011)
// ============================================================================
function renderManagedServerView() {
  const profile = state.selectedProfile;
  const provisionable = isProfileProvisionable(profile.id);
  const ms = state.managedServer;
  const managedSpec = MANAGED_SERVER_STATE_SPEC[ms.state] || MANAGED_SERVER_STATE_SPEC["Not set up"];
  const managedBadgeClass = managedSpec.token;
  const stateMsg = MANAGED_SERVER_STATE_MESSAGES[ms.state] || MANAGED_SERVER_STATE_MESSAGES["Not set up"];
  const failureGuidance = ms.failureCode ? lookupErrorGuidance(ms.failureCode) : null;

  const isInProgress =
    ms.state === "Downloading" ||
    ms.state === "Verifying" ||
    ms.state === "Configuring" ||
    ms.state === "Starting" ||
    ms.state === "Updating";

  const stepMeta = [
    { key: "download", label: "1. Download", detail: "Official server.jar + JRE 21" },
    { key: "verify", label: "2. Verify SHA-256", detail: "Checked before execution (SRV-002)" },
    { key: "configure", label: "3. Configure", detail: "127.0.0.1 · RCON & Query off" },
    { key: "start", label: "4. Start Process", detail: "Windows Job Object (SRV-007)" },
  ];

  return `
    <div class="sheet-header">
      <div class="sheet-header__titles">
        <h2 class="sheet-title">Managed Server Setup (S12) · ${profile.name}</h2>
        <p class="sheet-subtitle">
          Provision, verify, back up, and update a local server without editing configuration files by hand (<code data-ui>SRV-001</code>, <code data-ui>SRV-006</code>, <code data-ui>SRV-009</code>).
        </p>
      </div>
      <div class="sheet-header__actions">
        <button type="button" id="btn-back-to-local-config" class="gtt-btn gtt-btn-secondary">
          Manual Server Config (S5)
        </button>
        <button type="button" id="btn-back-to-dash-from-managed" class="gtt-btn gtt-btn-secondary">
          Back to Dashboard
        </button>
      </div>
    </div>

    ${
      !provisionable
        ? `
      <section id="managed-not-provisionable-card" class="non-loopback-warning" role="region" aria-label="Manual configuration required">
        <div style="font-size: 15px; font-weight: 600;">
          ${profile.name} uses manual server configuration (<code data-ui>SRV-003</code>)
        </div>
        <p style="font-size: 13px; color: var(--muted);">
          Automatic server provisioning is only offered for profiles on the verified provisionable allow-list (<code data-ui>minecraft-java</code>). Use Configure Server (S5) to point to your manually started ${profile.name} server.
        </p>
      </section>
    `
        : ""
    }

    <!-- 4th Separate Status Indicator: Managed Server (SRV-006, DESIGN.md §6) -->
    <section class="managed-status-bar" role="region" aria-label="Managed server status indicator">
      <div class="kpi" id="indicator-managed-server" data-indicator="managed-server" data-state="${ms.state}" data-token="${managedBadgeClass}" aria-live="${managedSpec.ariaLive}">
        <div class="kpi__label">
          <span>Managed Server Status (4th Indicator · <code data-ui>SRV-006</code>)</span>
        </div>
        <div class="kpi__value-row">
          <div class="indicator-badge ${managedBadgeClass}">
            <span class="status-icon ${managedBadgeClass}" data-icon-shape="${managedSpec.iconShape}"></span>
            <span class="kpi__value" data-role="indicator-value">${ms.state}</span>
          </div>
          <span class="pill ${managedSpec.pill}">v${ms.currentVersion}</span>
        </div>
        <div class="kpi__sub" data-role="managed-state-headline">${stateMsg.headline}</div>
      </div>

      <div class="kpi">
        <div class="kpi__label">
          <span>Reviewed State Cause &amp; Next Action</span>
        </div>
        <div style="font-size: 13px; color: var(--text);" data-role="managed-state-cause">
          <strong>Cause:</strong> ${failureGuidance ? failureGuidance.cause : stateMsg.cause}
        </div>
        <div style="font-size: 13px; color: var(--muted);" data-role="managed-state-action">
          <strong>Next step:</strong> ${failureGuidance ? failureGuidance.action : stateMsg.action}
        </div>
      </div>
    </section>

    <!-- Honest Cancellation Summary if Cancelled (DESIGN.md §8, user-flows.md §5 step 3) -->
    ${
      ms.cancelSummary
        ? `
      <div id="managed-cancel-summary" class="disclosure-callout" role="status">
        <strong>Provisioning Cancelled:</strong> ${ms.cancelSummary}
      </div>
    `
        : ""
    }

    <!-- Crash or Failure Banner with Log Excerpt Behind Details (DESIGN.md §6, user-flows.md §5 step 6) -->
    ${
      ms.state === "Crashed" || ms.state === "Failed"
        ? `
      <section id="managed-failure-banner" class="non-loopback-warning" role="alert" aria-label="Managed server failure details">
        <div style="font-size: 15px; font-weight: 600;">
          ${failureGuidance ? failureGuidance.title : stateMsg.headline}
          ${ms.state === "Crashed" ? `(Restart attempt ${ms.restartAttempt} / ${ms.maxRestartAttempts})` : ""}
        </div>
        <p style="font-size: 13px;"><strong>Cause:</strong> ${failureGuidance ? failureGuidance.cause : stateMsg.cause}</p>
        <p style="font-size: 13px;"><strong>Action:</strong> ${failureGuidance ? failureGuidance.action : stateMsg.action}</p>
        ${
          ms.artifactDiscarded
            ? `<p style="font-size: 13px; font-weight: 600;">Security guarantee (SRV-002): The unverified download artifact was discarded and never executed.</p>`
            : ""
        }
        ${
          ms.state === "Crashed" || ms.failureCode === "srv_process_crashed" || ms.failureCode === "srv_restart_limit_reached"
            ? `
          <details id="managed-crash-details">
            <summary style="cursor: pointer; font-weight: 600; font-size: 13px;">Details · Show crash log excerpt</summary>
            <pre style="margin-top: 8px; padding: 10px; background: var(--surface); border: 1px solid var(--border); border-radius: 6px; font-size: 12px; overflow-x: auto;">${ms.crashLogExcerpt}</pre>
          </details>
        `
            : ""
        }
      </section>
    `
        : ""
    }

    <div class="workspace-grid">
      <!-- Left 7 Columns: EULA Gate, Multi-Step Provisioning Progress & Generated Config Summary -->
      <div class="col-7" style="display: flex; flex-direction: column; gap: 16px;">
        <div class="panel">
          <div class="panel__head">
            <div>
              <h3 class="panel__title">1. Host EULA Confirmation &amp; Provisioning Steps (<code data-ui>SRV-001</code>, <code data-ui>SRV-002</code>)</h3>
              <span class="panel__sub">The app never accepts terms on your behalf and never executes unverified binaries</span>
            </div>
            <span class="pill ${ms.termsAccepted ? "pill--ok" : "pill--warn"}">
              ${ms.termsAccepted ? "EULA Confirmed by Host" : "EULA Confirmation Required"}
            </span>
          </div>

          <label class="checkbox-label" for="checkbox-accept-eula">
            <input type="checkbox"
                   id="checkbox-accept-eula"
                   ${ms.termsAccepted ? "checked" : ""} />
            <span>
              I have read and agree to the official Minecraft End User License Agreement (<code data-ui>${ms.eulaUrl}</code>)
            </span>
          </label>

          <!-- Discrete 4-Step Progress (Download -> Verify -> Configure -> Start, DESIGN.md §8) -->
          <div class="provisioning-stepper" role="list" aria-label="Provisioning steps">
            ${stepMeta
              .map((s) => {
                const st = ms.stepStatuses[s.key] || "pending";
                let stepClass = "";
                if (st === "active") stepClass = "is-active";
                if (st === "completed") stepClass = "is-completed";
                if (st === "failed") stepClass = "is-failed";
                return `
                  <div class="provisioning-step ${stepClass}" data-step-key="${s.key}" data-step-status="${st}" role="listitem">
                    <div style="font-size: 13px; font-weight: 600;">${s.label}</div>
                    <div style="font-size: 12px; color: var(--muted);">${s.detail}</div>
                    <div style="font-size: 12px; font-weight: 600; text-transform: uppercase;">${st}</div>
                  </div>
                `;
              })
              .join("")}
          </div>

          <div class="progress-track" role="progressbar" aria-valuenow="${ms.progressPercent}" aria-valuemin="0" aria-valuemax="100" aria-label="Provisioning progress">
            <div class="progress-fill" style="width: ${ms.progressPercent}%;"></div>
          </div>

          <div style="display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-wrap: wrap;">
            <span style="font-size: 13px; color: var(--muted);">
              Progress: <strong>${ms.progressPercent}%</strong> · Loopback listen address: <code data-ui>${ms.listenAddress}</code>
            </span>
            <div style="display: flex; gap: 8px; flex-wrap: wrap;">
              ${
                isInProgress
                  ? `
                <button type="button" id="btn-advance-provisioning" class="gtt-btn gtt-btn-secondary gtt-btn-sm">
                  Next Step
                </button>
                <button type="button" id="btn-cancel-provisioning" class="gtt-btn gtt-btn-destructive gtt-btn-sm">
                  Cancel Provisioning
                </button>
              `
                  : ""
              }
              ${
                ms.state !== "Running"
                  ? `
                <button type="button" id="btn-start-provisioning" class="gtt-btn gtt-btn-primary" ${!provisionable ? "disabled" : ""}>
                  Provision &amp; Start Server
                </button>
              `
                  : `
                <button type="button" id="btn-stop-managed-server" class="gtt-btn gtt-btn-destructive">
                  Stop Managed Server
                </button>
              `
              }
            </div>
          </div>
        </div>

        <!-- Generated Safe Configuration Summary (SRV-004, user-flows.md §5 step 4) -->
        <div class="panel panel--flush">
          <div class="panel__head">
            <div>
              <h3 class="panel__title">Generated Safe Server Configuration (<code data-ui>SRV-004</code>)</h3>
              <span class="panel__sub">Loopback bound (<code data-ui>127.0.0.1</code>); RCON and Query disabled by default for Java Edition</span>
            </div>
            <span class="pill pill--ok">server.properties</span>
          </div>
          <div class="table-scroll">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Setting</th>
                  <th>Generated Value</th>
                  <th>Security Rationale</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td><code data-ui>server-ip</code></td>
                  <td><code data-ui>${ms.generatedConfig.serverIp}</code></td>
                  <td>Loopback only; never exposed on LAN without explicit host action (INV-1, SRV-004)</td>
                </tr>
                <tr>
                  <td><code data-ui>server-port</code></td>
                  <td><code data-ui>${ms.generatedConfig.serverPort}</code></td>
                  <td>Matches profile default TCP mapping</td>
                </tr>
                <tr>
                  <td><code data-ui>enable-rcon</code></td>
                  <td><code data-ui>${String(ms.generatedConfig.enableRcon)}</code></td>
                  <td>Remote admin console disabled by default (GAME-002, GAME-008)</td>
                </tr>
                <tr>
                  <td><code data-ui>enable-query</code></td>
                  <td><code data-ui>${String(ms.generatedConfig.enableQuery)}</code></td>
                  <td>UT3 status query disabled by default (GAME-008)</td>
                </tr>
                <tr>
                  <td><code data-ui>online-mode</code></td>
                  <td><code data-ui>${String(ms.generatedConfig.onlineMode)}</code></td>
                  <td>Mojang session authentication stays enabled</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </div>

      <!-- Right 5 Columns: World Backups, Restore & Version Update (SRV-009) -->
      <div class="col-5" style="display: flex; flex-direction: column; gap: 16px;">
        <div class="panel">
          <div class="panel__head">
            <div>
              <h3 class="panel__title">Server Version &amp; Update (<code data-ui>SRV-009</code>)</h3>
              <span class="panel__sub">Installed: <code data-ui>v${ms.currentVersion}</code> · Available: <code data-ui>v${ms.availableUpdateVersion}</code></span>
            </div>
            <button type="button" id="btn-update-managed-server" class="gtt-btn gtt-btn-primary gtt-btn-sm">
              Update to v${ms.availableUpdateVersion}
            </button>
          </div>
          <p style="font-size: 13px; color: var(--muted);">
            Updating while a tunnel is active asks for confirmation first because restarting the local game server disconnects active players. An automatic world backup is created before every update.
          </p>
        </div>

        <div class="panel panel--flush">
          <div class="panel__head">
            <div>
              <h3 class="panel__title">World Backups &amp; Restore (<code data-ui>SRV-009</code>)</h3>
              <span class="panel__sub">${ms.backups.length} snapshots saved locally</span>
            </div>
            <button type="button" id="btn-create-world-backup" class="gtt-btn gtt-btn-secondary gtt-btn-sm">
              Create Backup Now
            </button>
          </div>
          <div class="table-scroll">
            <table class="data-table" id="managed-backups-table">
              <thead>
                <tr>
                  <th>Backup ID</th>
                  <th>Label &amp; Version</th>
                  <th class="num">Size</th>
                  <th class="num">Action</th>
                </tr>
              </thead>
              <tbody>
                ${ms.backups
                  .map(
                    (b) => `
                  <tr data-backup-id="${b.id}">
                    <td><code data-ui>${b.id}</code></td>
                    <td>
                      <div style="font-weight: 600;">${b.label}</div>
                      <div style="font-size: 12px; color: var(--muted);">v${b.version} · ${b.createdAt}</div>
                    </td>
                    <td class="num">${b.sizeMb.toFixed(1)} MB</td>
                    <td class="num">
                      <button type="button" class="gtt-btn gtt-btn-secondary gtt-btn-sm btn-restore-backup" data-backup-id="${b.id}">
                        Restore
                      </button>
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
    </div>

    <!-- Confirmation Modal when updating managed server while tunnel is active (SRV-009, DESIGN.md §7) -->
    <div id="managed-update-confirmation-modal" class="modal-overlay ${state.isManagedUpdateModalOpen ? "active" : ""}">
      <div class="modal-card" role="dialog" aria-modal="true" aria-labelledby="managed-update-modal-title">
        <h3 id="managed-update-modal-title" class="gtt-card-title">Update Server While Tunnel Is Active?</h3>
        <p style="color: var(--muted); line-height: 1.5;">
          Your tunnel is currently <strong>${state.tunnelState}</strong>. Updating the managed server to <strong>v${state.pendingManagedUpdateVersion || ms.availableUpdateVersion}</strong> will restart the server process and disconnect connected players. A world backup will be created automatically before updating.
        </p>
        <div class="modal-actions">
          <button type="button" id="btn-cancel-managed-update" class="gtt-btn gtt-btn-secondary">
            Cancel Update
          </button>
          <button type="button" id="btn-confirm-managed-update" class="gtt-btn gtt-btn-destructive">
            Backup, Restart &amp; Update
          </button>
        </div>
      </div>
    </div>
  `;
}

function bindManagedServerEvents() {
  document.getElementById("btn-back-to-local-config")?.addEventListener("click", () => {
    navigateTo("local-config");
  });

  document.getElementById("btn-back-to-dash-from-managed")?.addEventListener("click", () => {
    navigateTo("dashboard");
  });

  document.getElementById("checkbox-accept-eula")?.addEventListener("change", (e) => {
    acceptManagedServerTerms(e.target.checked);
  });

  document.getElementById("btn-start-provisioning")?.addEventListener("click", () => {
    startManagedServerProvisioning();
  });

  document.getElementById("btn-advance-provisioning")?.addEventListener("click", () => {
    advanceManagedServerProvisioningStep();
  });

  document.getElementById("btn-cancel-provisioning")?.addEventListener("click", () => {
    cancelManagedServerProvisioning();
  });

  document.getElementById("btn-stop-managed-server")?.addEventListener("click", () => {
    stopManagedServer();
  });

  document.getElementById("btn-create-world-backup")?.addEventListener("click", () => {
    createManagedWorldBackup("Manual host snapshot");
  });

  document.querySelectorAll(".btn-restore-backup").forEach((btn) => {
    btn.addEventListener("click", () => {
      const bkpId = btn.getAttribute("data-backup-id");
      if (bkpId) restoreManagedWorldBackup(bkpId);
    });
  });

  document.getElementById("btn-update-managed-server")?.addEventListener("click", () => {
    requestManagedServerUpdate(state.managedServer.availableUpdateVersion);
  });

  document.getElementById("btn-cancel-managed-update")?.addEventListener("click", () => {
    cancelManagedServerUpdate();
  });

  document.getElementById("btn-confirm-managed-update")?.addEventListener("click", () => {
    confirmManagedServerUpdate();
  });
}

// ============================================================================
// T-086: Keyboard Navigation, Escape Dismissal & Focus Trap (UX-004, DESIGN.md §§7, 9)
// ============================================================================
function handleGlobalKeyDown(event) {
  const key = event?.key;
  if (!key) return false;

  if (key === "Escape") {
    if (state.isStopModalOpen) {
      event.preventDefault?.();
      cancelStopTunnel();
      return true;
    }
    if (state.isManagedUpdateModalOpen) {
      event.preventDefault?.();
      cancelManagedServerUpdate();
      return true;
    }
    closeMobileDrawer();
    return true;
  }

  // Trap Tab focus inside active modal dialog (DESIGN.md §7)
  if (key === "Tab" && (state.isStopModalOpen || state.isManagedUpdateModalOpen)) {
    const pair = state.isStopModalOpen
      ? ["btn-cancel-stop", "btn-confirm-stop"]
      : ["btn-cancel-managed-update", "btn-confirm-managed-update"];
    event.preventDefault?.();
    const currentIdx = pair.indexOf(state.activeFocusedElementId);
    const nextIdx = currentIdx === 0 ? 1 : 0;
    state.activeFocusedElementId = pair[nextIdx];
    document.getElementById(state.activeFocusedElementId)?.focus?.();
    return true;
  }

  return false;
}

// Keyboard-only control dispatcher for accessibility testing and keyboard activation (UX-004)
function dispatchKeyAction(targetId, key = "Enter") {
  if (key === "Escape" || key === "Tab") {
    return handleGlobalKeyDown({ key, preventDefault: () => {} });
  }
  if (key !== "Enter" && key !== " ") {
    return false;
  }

  state.activeFocusedElementId = targetId;

  // Sidebar navigation via keyboard
  if (targetId.startsWith("nav:")) {
    const view = targetId.slice(4);
    navigateTo(view);
    return true;
  }

  // Profile selection on S4 Choose Game via Enter or Space
  if (targetId.startsWith("profile-card:")) {
    const profileId = targetId.slice("profile-card:".length);
    const found = SHIPPED_PROFILES.find((p) => p.id === profileId);
    if (found) {
      state.selectedProfile = found;
      state.mappingConfigs = createInitialMappingConfigs(found);
      state.validationErrors = {};
      state.probeResults = {};
      state.publicAddress = `fra1.relay.gtt.gg:${found.mappings[0].defaultPort}`;
      navigateTo("local-config");
      return true;
    }
  }

  switch (targetId) {
    // S1 Onboarding -> S2 Sign-In Handoff -> S4 Choose Game
    case "btn-start-signin":
      navigateTo("signin-handoff");
      return true;
    case "btn-choose-game":
      navigateTo("choose-game");
      return true;
    case "btn-simulate-auth-success":
      state.isSignedIn = true;
      navigateTo("choose-game");
      return true;
    case "btn-cancel-auth":
      navigateTo("onboarding");
      return true;

    // S4 Choose Game -> S5 Local Config
    case "btn-proceed-to-config":
      navigateTo("local-config");
      return true;

    // S5 Local Config -> S7 Dashboard or S12 Managed Server
    case "btn-save-and-proceed":
      if (validateAllMappings(state.selectedProfile)) {
        const primaryPort =
          state.mappingConfigs[state.selectedProfile.mappings[0].key]?.port ||
          state.selectedProfile.mappings[0].defaultPort;
        state.publicAddress = `fra1.relay.gtt.gg:${primaryPort}`;
        navigateTo("dashboard");
        return true;
      }
      render();
      return false;
    case "btn-setup-managed-server":
      if (isProfileProvisionable(state.selectedProfile.id)) {
        navigateTo("managed-server");
        return true;
      }
      return false;

    // S7 Dashboard: Start, Copy Address, Stop with confirmation
    case "btn-start-tunnel":
      setTunnelState("Connected");
      setLocalServerState("Reachable");
      setPlayersSeen(ACTIVE_GUEST_FLOWS.length);
      return true;
    case "btn-copy-address":
      showToast("Public address copied to clipboard!");
      return true;
    case "btn-stop-tunnel":
      requestStopTunnel();
      return true;
    case "btn-cancel-stop":
      cancelStopTunnel();
      return true;
    case "btn-confirm-stop":
      confirmStopTunnel();
      return true;

    // S8 Invitations: Create, Copy Secret, Dismiss
    case "btn-create-invitation":
      createInvitation({ expiresInSeconds: 3600, maxClaims: 4 });
      return true;
    case "btn-copy-secret":
      showToast("One-time invitation secret copied to clipboard");
      return true;
    case "btn-dismiss-secret":
      dismissOneTimeSecret();
      return true;

    // S12 Managed Server
    case "checkbox-accept-eula":
      acceptManagedServerTerms(!state.managedServer.termsAccepted);
      return true;
    case "btn-start-provisioning":
      startManagedServerProvisioning();
      return true;
    case "btn-update-managed-server":
      requestManagedServerUpdate();
      return true;
    case "btn-cancel-managed-update":
      cancelManagedServerUpdate();
      return true;
    case "btn-confirm-managed-update":
      confirmManagedServerUpdate();
      return true;
    case "btn-stop-managed-server":
      stopManagedServer();
      return true;
    default:
      return false;
  }
}

window.addEventListener("keydown", handleGlobalKeyDown);

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

// Export symbols onto globalThis for Node VM unit testing (T-080, T-082, T-083, T-084, T-085, T-086, T-088)
Object.assign(globalThis, {
  SHIPPED_PROFILES,
  SENSITIVE_LOCAL_PORTS,
  OBSERVED_TRAFFIC_LABEL,
  TUNNEL_STATE_SPEC,
  LOCAL_SERVER_STATE_SPEC,
  PROVISIONABLE_PROFILE_IDS,
  MANAGED_SERVER_STATE_SPEC,
  MANAGED_SERVER_STATE_MESSAGES,
  MANAGED_SERVER_FAILURE_CODES,
  GAME_JOIN_INSTRUCTIONS,
  REQUIRED_CONTRACT_ERROR_CODES,
  REQUIRED_ENGINE_FAILURE_CLASSES,
  ERROR_MESSAGE_CATALOG,
  createInitialMappingConfigs,
  isLoopbackHost,
  classifyProhibitedCategory,
  isValidHostString,
  validateSingleMapping,
  validateAllMappings,
  mapEngineTunnelState,
  setTunnelState,
  setLocalServerState,
  setPlayersSeen,
  formatPlayersSeenText,
  requestStopTunnel,
  cancelStopTunnel,
  confirmStopTunnel,
  isProfileProvisionable,
  setManagedServerState,
  acceptManagedServerTerms,
  startManagedServerProvisioning,
  advanceManagedServerProvisioningStep,
  cancelManagedServerProvisioning,
  stopManagedServer,
  createManagedWorldBackup,
  restoreManagedWorldBackup,
  requestManagedServerUpdate,
  cancelManagedServerUpdate,
  confirmManagedServerUpdate,
  setConfirmedInGamePresence,
  generateInvitationSecret,
  createInvitation,
  listInvitations,
  dismissOneTimeSecret,
  revokeInvitation,
  revokeAdmission,
  claimInvitation,
  lookupErrorGuidance,
  evaluateTroubleshooterChecks,
  exportRedactedDiagnostics,
  handleGlobalKeyDown,
  dispatchKeyAction,
  navigateTo,
  render,
  renderOnboardingView,
  renderSignInHandoffView,
  renderChooseGameView,
  renderLocalConfigView,
  renderDashboardView,
  renderInvitationsView,
  renderTroubleshooterView,
  renderManagedServerView,
  state,
});
