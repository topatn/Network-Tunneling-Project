/**
 * Tunnel Dashboard Three-Layer Status & Stop Confirmation Test Suite
 * Tasks & Requirements: T-083, FR-010, FR-012, UX-002, UX-006, DESIGN.md §6
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

const APP_JS_PATH = path.resolve(__dirname, '..', 'src', 'app.js');

console.log('Testing Tunnel Dashboard Three-Layer Status & Stop Confirmation (T-083)...');

assert(fs.existsSync(APP_JS_PATH), 'app.js must exist');
const appJsCode = fs.readFileSync(APP_JS_PATH, 'utf8');

// Track rendered HTML inside main-content
const mainContentMock = { innerHTML: '' };
const viewTitleMock = { textContent: '' };
const topbarScopeMock = { textContent: '' };
const navBadgeDashMock = { textContent: '' };
const navBadgeMapMock = { textContent: '' };

const elementsById = {
  'main-content': mainContentMock,
  'view-title': viewTitleMock,
  'topbar-scope': topbarScopeMock,
  'nav-badge-dashboard': navBadgeDashMock,
  'nav-badge-mappings': navBadgeMapMock,
};

const windowMock = {
  addEventListener: () => {},
  document: {
    addEventListener: () => {},
    getElementById: (id) => elementsById[id] || null,
    querySelectorAll: () => [],
    querySelector: () => null,
    createElement: () => ({
      className: '',
      setAttribute: () => {},
      textContent: '',
      remove: () => {},
    }),
    documentElement: { setAttribute: () => {}, removeAttribute: () => {} },
  },
  localStorage: { getItem: () => null, setItem: () => {} },
};

const sandbox = {
  window: windowMock,
  document: windowMock.document,
  localStorage: windowMock.localStorage,
  console,
  setTimeout,
};

vm.createContext(sandbox);
vm.runInContext(appJsCode, sandbox);

const {
  OBSERVED_TRAFFIC_LABEL,
  TUNNEL_STATE_SPEC,
  LOCAL_SERVER_STATE_SPEC,
  mapEngineTunnelState,
  setTunnelState,
  setLocalServerState,
  setPlayersSeen,
  requestStopTunnel,
  cancelStopTunnel,
  confirmStopTunnel,
  renderDashboardView,
  state,
} = sandbox;

// Helper to extract indicator attributes from rendered HTML
function extractIndicatorAttributes(html, indicatorId) {
  const regex = new RegExp(
    `<div class="kpi" id="${indicatorId}"\\s+data-indicator="([^"]+)"\\s+data-state="([^"]+)"\\s+data-token="([^"]+)"\\s+aria-live="([^"]+)"`,
    'i'
  );
  const match = regex.exec(html);
  assert(match, `Indicator #${indicatorId} must be present in rendered dashboard HTML`);
  return {
    indicator: match[1],
    state: match[2],
    token: match[3],
    ariaLive: match[4],
  };
}

// 1. Verify Engine State Machine to UI Label Mappings (DESIGN.md §6 & contracts.md §12)
const expectedEngineMappings = [
  ['Idle', 'Not running', 'neutral', 'hollow-circle', 'off'],
  ['Stopped', 'Not running', 'neutral', 'hollow-circle', 'off'],
  ['Connecting', 'Connecting', 'info', 'spinner', 'polite'],
  ['Authenticating', 'Connecting', 'info', 'spinner', 'polite'],
  ['Active', 'Connected', 'ok', 'check-circle', 'polite'],
  ['Degraded', 'Degraded', 'warn', 'triangle', 'polite'],
  ['Reconnecting', 'Reconnecting', 'warn', 'circular-arrows', 'polite'],
  ['Failed', 'Failed', 'error', 'octagon-cross', 'assertive'],
];

for (const [engineState, expectedLabel, expectedToken, expectedShape, expectedPoliteness] of expectedEngineMappings) {
  const label = mapEngineTunnelState(engineState);
  assert.strictEqual(label, expectedLabel, `Engine state ${engineState} must map to "${expectedLabel}"`);
  const spec = TUNNEL_STATE_SPEC[label];
  assert(spec, `TUNNEL_STATE_SPEC must define "${label}"`);
  assert.strictEqual(spec.token, expectedToken, `Token mismatch for ${label}`);
  assert.strictEqual(spec.iconShape, expectedShape, `Icon shape mismatch for ${label}`);
  assert.strictEqual(spec.ariaLive, expectedPoliteness, `aria-live mismatch for ${label}`);
}
console.log('[OK] DESIGN.md §6: All 6 tunnel states, semantic tokens, icon shapes, and aria-live rules verified.');

// 2. Verify Independent Transitions of Indicator 1: Tunnel Connection (FR-010, UX-002)
setLocalServerState('Not reachable');
setPlayersSeen(0);

const tunnelSequence = ['Not running', 'Connecting', 'Connected', 'Degraded', 'Reconnecting', 'Failed'];
for (const targetTunnelState of tunnelSequence) {
  setTunnelState(targetTunnelState);
  const html = renderDashboardView();

  const tunnelAttr = extractIndicatorAttributes(html, 'indicator-tunnel');
  const serverAttr = extractIndicatorAttributes(html, 'indicator-local-server');
  const playersAttr = extractIndicatorAttributes(html, 'indicator-players-seen');

  assert.strictEqual(tunnelAttr.state, targetTunnelState, `Tunnel indicator must show "${targetTunnelState}"`);
  assert.strictEqual(tunnelAttr.token, TUNNEL_STATE_SPEC[targetTunnelState].token);
  assert.strictEqual(tunnelAttr.ariaLive, TUNNEL_STATE_SPEC[targetTunnelState].ariaLive);

  // Prove Local Server and Players Seen did NOT change when Tunnel state changed (UX-002)
  assert.strictEqual(serverAttr.state, 'Not reachable', 'Local server indicator must remain "Not reachable" while tunnel state changes');
  assert.strictEqual(serverAttr.token, 'error');
  assert.strictEqual(playersAttr.state, 'No traffic seen', 'Players seen indicator must remain "No traffic seen" while tunnel state changes');
  assert.strictEqual(playersAttr.token, 'neutral');
}
console.log('[OK] FR-010 & UX-002: Tunnel Connection indicator transitions across all 6 states without affecting Local Server or Players Seen.');

// 3. Verify Independent Transitions of Indicator 2: Local Server Reachability (FR-010, FR-011, UX-002)
setTunnelState('Connected');
setPlayersSeen(2);

const serverSequence = [
  ['Reachable', 'ok'],
  ['Not reachable', 'error'],
  ['Checking', 'info'],
  ['Unknown', 'neutral'], // UDP without a query probe must show Unknown (neutral), never Reachable (FR-011)
];

for (const [targetServerState, expectedToken] of serverSequence) {
  setLocalServerState(targetServerState);
  const html = renderDashboardView();

  const tunnelAttr = extractIndicatorAttributes(html, 'indicator-tunnel');
  const serverAttr = extractIndicatorAttributes(html, 'indicator-local-server');
  const playersAttr = extractIndicatorAttributes(html, 'indicator-players-seen');

  assert.strictEqual(serverAttr.state, targetServerState, `Local server indicator must show "${targetServerState}"`);
  assert.strictEqual(serverAttr.token, expectedToken, `Local server token must be "${expectedToken}"`);

  // Prove Tunnel and Players Seen did NOT change when Local Server state changed
  assert.strictEqual(tunnelAttr.state, 'Connected', 'Tunnel indicator must remain "Connected" when local server changes');
  assert.strictEqual(playersAttr.state, '2 guests seen', 'Players seen indicator must remain "2 guests seen" when local server changes');
}
console.log('[OK] FR-010 & FR-011: Local Server indicator transitions across Reachable, Not reachable, Checking, and Unknown independently.');

// 4. Verify Independent Transitions of Indicator 3: Guest Traffic Observed (FR-010, FR-012, UX-002)
setTunnelState('Connected');
setLocalServerState('Not reachable');

const playersSequence = [
  [0, 'No traffic seen', 'neutral'],
  [1, '1 guest seen', 'ok'],
  [5, '5 guests seen', 'ok'],
  [null, 'Not available', 'neutral'],
];

for (const [inputVal, expectedText, expectedToken] of playersSequence) {
  setPlayersSeen(inputVal);
  const html = renderDashboardView();

  const tunnelAttr = extractIndicatorAttributes(html, 'indicator-tunnel');
  const serverAttr = extractIndicatorAttributes(html, 'indicator-local-server');
  const playersAttr = extractIndicatorAttributes(html, 'indicator-players-seen');

  assert.strictEqual(playersAttr.state, expectedText, `Players indicator must display "${expectedText}"`);
  assert.strictEqual(playersAttr.token, expectedToken, `Players token must be "${expectedToken}"`);
  assert(
    html.includes(OBSERVED_TRAFFIC_LABEL),
    'Dashboard must explicitly label players count as observed relay traffic, not confirmed in-game presence (FR-012)'
  );

  // Prove Tunnel and Local Server did NOT change when Players Seen changed
  assert.strictEqual(tunnelAttr.state, 'Connected', 'Tunnel state must remain "Connected"');
  assert.strictEqual(serverAttr.state, 'Not reachable', 'Local server state must remain "Not reachable"');
}
console.log('[OK] FR-012 & UX-002: Guest Traffic Observed indicator updates independently and displays explicit observed-traffic disclosure.');

// 5. Negative Input Validation Tests (TEST-001)
assert.throws(
  () => setLocalServerState('InvalidState'),
  /Invalid local server state/,
  'setLocalServerState must reject unknown state strings'
);
assert.throws(
  () => setPlayersSeen(-1),
  /Invalid playersSeenCount/,
  'setPlayersSeen must reject negative player counts'
);
assert.throws(
  () => setPlayersSeen(2.5),
  /Invalid playersSeenCount/,
  'setPlayersSeen must reject non-integer player counts'
);
console.log('[OK] TEST-001: Negative and malformed status indicator inputs are rejected.');

// 6. Verify Stop Confirmation Gate (UX-006)
// Case 6a: Stopping when players ARE seen (playersSeenCount > 0) MUST open confirmation modal and NOT stop tunnel
setTunnelState('Connected');
setLocalServerState('Reachable');
setPlayersSeen(3);

const stopWithPlayersResult = requestStopTunnel();
assert.strictEqual(stopWithPlayersResult.confirmedImmediately, false, 'Must not stop immediately when guests are seen');
assert.strictEqual(stopWithPlayersResult.modalOpened, true, 'Must open confirmation modal when guests are seen (UX-006)');
assert.strictEqual(state.isStopModalOpen, true, 'state.isStopModalOpen must be true');
assert.strictEqual(state.tunnelState, 'Connected', 'Tunnel must remain Connected while awaiting confirmation');

let modalHtml = renderDashboardView();
assert(
  modalHtml.includes('id="stop-confirmation-modal" class="modal-overlay active"'),
  'Confirmation modal must have .active class when open'
);
assert(
  modalHtml.includes('3 active guests'),
  'Confirmation modal must state the active guest count'
);

// Case 6b: Canceling the stop confirmation modal keeps the tunnel running
cancelStopTunnel();
assert.strictEqual(state.isStopModalOpen, false, 'Modal must close on cancel');
assert.strictEqual(state.tunnelState, 'Connected', 'Tunnel must stay Connected after cancel');
assert.strictEqual(state.playersSeenCount, 3, 'Players seen count must remain 3 after cancel');

// Case 6c: Confirming the stop modal stops the tunnel and clears players seen
requestStopTunnel();
assert.strictEqual(state.isStopModalOpen, true);
confirmStopTunnel();
assert.strictEqual(state.isStopModalOpen, false, 'Modal must close on confirm');
assert.strictEqual(state.tunnelState, 'Not running', 'Tunnel must transition to "Not running" after confirm');
assert.strictEqual(state.playersSeenCount, 0, 'Players seen must reset to 0 after tunnel stops');

// Case 6d: Stopping when NO players are seen (playersSeenCount === 0) stops immediately WITHOUT confirmation modal
setTunnelState('Connected');
setPlayersSeen(0);
const stopWithoutPlayersResult = requestStopTunnel();
assert.strictEqual(stopWithoutPlayersResult.confirmedImmediately, true, 'Must stop immediately when 0 players seen (UX-006)');
assert.strictEqual(stopWithoutPlayersResult.modalOpened, false, 'Must NOT open confirmation modal when 0 players seen (UX-006)');
assert.strictEqual(state.isStopModalOpen, false, 'state.isStopModalOpen must remain false');
assert.strictEqual(state.tunnelState, 'Not running', 'Tunnel must immediately become "Not running"');

modalHtml = renderDashboardView();
assert(
  !modalHtml.includes('class="modal-overlay active"'),
  'Confirmation modal must not be active when stopping with 0 players'
);
console.log('[OK] UX-006: Stop confirmation appears if and only if observed players > 0.');

console.log('\n===================================================================');
console.log(' ALL TUNNEL DASHBOARD THREE-LAYER STATUS TESTS PASSED (EXIT 0)     ');
console.log('===================================================================');
