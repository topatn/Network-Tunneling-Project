/**
 * Error Message Catalog & Guided 6-Step Troubleshooter Test Suite
 * Tasks & Requirements: T-085, FR-015, UX-003, FR-013, PRIV-002, contracts.md §3, user-flows.md §§11–12
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

const ROOT_DIR = path.resolve(__dirname, '..', '..', '..');
const CONTRACTS_MD_PATH = path.join(ROOT_DIR, 'contracts.md');
const APP_JS_PATH = path.resolve(__dirname, '..', 'src', 'app.js');

console.log('Testing Error Message Catalog & Guided Troubleshooter (T-085)...');

assert(fs.existsSync(CONTRACTS_MD_PATH), 'contracts.md must exist');
assert(fs.existsSync(APP_JS_PATH), 'app.js must exist');

// 1. Parse contracts.md §3 directly to extract every authoritative error code from the table
const contractsMd = fs.readFileSync(CONTRACTS_MD_PATH, 'utf8');
const section3Match = /## 3\. Error model([\s\S]*?)## 4\./.exec(contractsMd);
assert(section3Match, 'contracts.md must contain section 3 Error model');

const codeRowRegex = /^\|\s*`([a-z_]+)`\s*\|\s*(\d{3})\s*\|/gm;
const parsedContractCodes = [];
let rowMatch;
while ((rowMatch = codeRowRegex.exec(section3Match[1])) !== null) {
  parsedContractCodes.push({
    code: rowMatch[1],
    httpStatus: Number(rowMatch[2]),
  });
}

assert.strictEqual(
  parsedContractCodes.length,
  12,
  `Expected 12 error codes in contracts.md §3, found ${parsedContractCodes.length}`
);

// Load app.js in VM sandbox
const appJsCode = fs.readFileSync(APP_JS_PATH, 'utf8');
const windowMock = {
  addEventListener: () => {},
  document: {
    addEventListener: () => {},
    getElementById: () => null,
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
  REQUIRED_CONTRACT_ERROR_CODES,
  REQUIRED_ENGINE_FAILURE_CLASSES,
  ERROR_MESSAGE_CATALOG,
  lookupErrorGuidance,
  evaluateTroubleshooterChecks,
  exportRedactedDiagnostics,
  renderTroubleshooterView,
  setTunnelState,
  setLocalServerState,
  setPlayersSeen,
  state,
} = sandbox;

// 2. Verify every code in contracts.md §3 has a reviewed entry with cause & action (UX-003, T-085 Done-when)
for (const { code, httpStatus } of parsedContractCodes) {
  assert(
    REQUIRED_CONTRACT_ERROR_CODES.includes(code),
    `REQUIRED_CONTRACT_ERROR_CODES must include ${code}`
  );
  const entry = ERROR_MESSAGE_CATALOG[code];
  assert(entry, `ERROR_MESSAGE_CATALOG missing entry for contracts.md §3 code: ${code}`);
  assert.strictEqual(entry.code, code);
  assert.strictEqual(entry.category, 'api');
  assert.strictEqual(entry.httpStatus, httpStatus, `HTTP status mismatch for ${code}`);
  assert(entry.title && entry.title.length >= 10, `${code} must have a reviewed plain-language title`);
  assert(entry.cause && entry.cause.length >= 20, `${code} must state a clear cause (UX-003)`);
  assert(entry.action && entry.action.length >= 20, `${code} must state a clear next action (UX-003)`);
  assert(entry.actionLabel && entry.actionLabel.length >= 4, `${code} must define an action button label`);
}
console.log(`[OK] contracts.md §3: All ${parsedContractCodes.length}/12 API error codes verified with cause and action.`);

// 3. Verify all Engine & Failure-Class codes required by FR-015 & user-flows.md §§11–12
const expectedEngineClasses = [
  'udp_blocked',
  'local_server_down',
  'local_server_wrong_bind',
  'optional_port_disabled',
  'token_expired',
  'handshake_failed',
  'grace_period_expired',
];

for (const code of expectedEngineClasses) {
  assert(
    REQUIRED_ENGINE_FAILURE_CLASSES.includes(code),
    `REQUIRED_ENGINE_FAILURE_CLASSES must include ${code}`
  );
  const entry = ERROR_MESSAGE_CATALOG[code];
  assert(entry, `ERROR_MESSAGE_CATALOG missing engine failure class: ${code}`);
  assert.strictEqual(entry.category, 'engine');
  assert(entry.title && entry.title.length >= 10, `${code} must have a reviewed title`);
  assert(entry.cause && entry.cause.length >= 20, `${code} must state a clear cause (FR-015, UX-003)`);
  assert(entry.action && entry.action.length >= 20, `${code} must state a clear next action (FR-015, UX-003)`);
}
console.log(`[OK] FR-015: All ${expectedEngineClasses.length}/7 engine failure classes verified with cause and action.`);

// 4. Verify lookupErrorGuidance handles strings, nested ErrorResponse objects, and unknown fallbacks
assert.strictEqual(lookupErrorGuidance('quota_exceeded').code, 'quota_exceeded');
assert.strictEqual(
  lookupErrorGuidance({ error: { code: 'relay_unavailable' } }).code,
  'relay_unavailable'
);
assert.strictEqual(
  lookupErrorGuidance('nonexistent_error_xyz').code,
  'internal',
  'Unknown error codes must safely fall back to internal guidance'
);

// Ensure no catalog entry contains secret patterns (contracts.md §3: "Error messages never echo secrets")
const serializedCatalog = JSON.stringify(ERROR_MESSAGE_CATALOG);
assert(!serializedCatalog.includes('gtt_inv_'), 'Catalog must never contain invitation secrets');
assert(!serializedCatalog.includes('Bearer '), 'Catalog must never contain bearer tokens');
console.log('[OK] lookupErrorGuidance and zero-secret catalog invariant verified.');

// 5. Verify 6-Step Guided Troubleshooter (user-flows.md §12)
// 5a: Healthy state
state.isSignedIn = true;
setTunnelState('Connected');
setLocalServerState('Reachable');
setPlayersSeen(3);

let steps = evaluateTroubleshooterChecks();
assert.strictEqual(steps.length, 6, 'Troubleshooter must run 6 ordered checks (user-flows.md §12)');
assert.strictEqual(steps[0].id, 'signed_in');
assert.strictEqual(steps[0].status, 'pass');
assert.strictEqual(steps[1].id, 'relay_reachable');
assert.strictEqual(steps[1].status, 'pass');
assert.strictEqual(steps[2].id, 'tunnel_active');
assert.strictEqual(steps[2].status, 'pass');
assert.strictEqual(steps[3].id, 'local_server_reachable');
assert.strictEqual(steps[3].status, 'pass');
assert.strictEqual(steps[4].id, 'guest_traffic_observed');
assert.strictEqual(steps[4].status, 'pass');
assert.strictEqual(steps[5].id, 'game_level_reminders');
assert.strictEqual(steps[5].status, 'info');
assert(
  steps[5].summary.includes('do not prove the game server accepted the player'),
  'Step 6 must state that observed traffic does not prove game acceptance (user-flows.md §12)'
);

// 5b: Local server down & Unknown UDP probe states
setLocalServerState('Not reachable');
steps = evaluateTroubleshooterChecks();
assert.strictEqual(steps[3].status, 'fail', 'Step 4 must fail when local server is Not reachable');

setLocalServerState('Unknown');
steps = evaluateTroubleshooterChecks();
assert.strictEqual(steps[3].status, 'unknown', 'Step 4 must report unknown when UDP probe does not exist');
assert(
  steps[3].summary.includes('Cannot be probed automatically'),
  'Unknown probe must instruct host to check the game server manually (user-flows.md §12)'
);

// 5c: Verify rendered Troubleshooter HTML includes all 19 catalog codes and 6 steps
setLocalServerState('Reachable');
const troubleHtml = renderTroubleshooterView();
for (const { code } of parsedContractCodes) {
  assert(
    troubleHtml.includes(`data-error-code="${code}"`),
    `Rendered catalog table must include row for ${code}`
  );
}
for (const code of expectedEngineClasses) {
  assert(
    troubleHtml.includes(`data-error-code="${code}"`),
    `Rendered catalog table must include row for ${code}`
  );
}
console.log('[OK] user-flows.md §12: 6-step guided troubleshooter and rendered catalog verified.');

// 6. Verify Redacted Diagnostics Export from Troubleshooter (FR-013, PRIV-002)
const bundle = exportRedactedDiagnostics();
const bundleJson = JSON.stringify(bundle);
assert.strictEqual(bundle.schema_version, 1);
assert.strictEqual(bundle.troubleshooter_steps.length, 6);
assert(!bundleJson.includes('gtt_inv_'), 'Diagnostics bundle must not contain invitation secrets');
assert(
  bundle.active_guest_subnets_masked.every((s) => s.endsWith('/24')),
  'All guest subnets in diagnostics bundle must be /24 masked (PRIV-002)'
);
console.log('[OK] FR-013 & PRIV-002: Redacted diagnostics export verified.');

console.log('\n===================================================================');
console.log(' ALL ERROR CATALOG & TROUBLESHOOTER TESTS PASSED (EXIT 0)          ');
console.log('===================================================================');
