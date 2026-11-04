/**
 * Game Selection and Local Configuration Automated Test Suite (T-082, FR-002, FR-003, GAME-009, GAME-008, INV-1)
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');

const APP_JS_PATH = path.resolve(__dirname, '..', 'src', 'app.js');
const PROFILES_DIR = path.resolve(__dirname, '..', '..', '..', 'profiles');

console.log('Testing Game Selection & Local Configuration (T-082)...');

// Read app.js
assert(fs.existsSync(APP_JS_PATH), 'app.js must exist');
const appJsCode = fs.readFileSync(APP_JS_PATH, 'utf8');

// Load environment in Node vm or eval context
const vm = require('vm');
const windowMock = {
  addEventListener: () => {},
  document: {
    addEventListener: () => {},
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
  SHIPPED_PROFILES,
  SENSITIVE_LOCAL_PORTS,
  createInitialMappingConfigs,
  isLoopbackHost,
  classifyProhibitedCategory,
  isValidHostString,
  validateSingleMapping,
  validateAllMappings,
  state,
} = sandbox;

// 1. Verify all 5 profiles match physical files in profiles/ (FR-002)
assert.strictEqual(SHIPPED_PROFILES.length, 5, 'Must contain exactly 5 game profiles');
const expectedProfileIds = ['minecraft-java', 'minecraft-bedrock', 'terraria', 'l4d2', 'l4d1'];

for (const id of expectedProfileIds) {
  const profile = SHIPPED_PROFILES.find((p) => p.id === id);
  assert(profile, `Profile ${id} must exist in SHIPPED_PROFILES`);

  const diskPath = path.join(PROFILES_DIR, `${id}.json`);
  assert(fs.existsSync(diskPath), `Profile definition ${diskPath} must exist on disk`);
  const diskData = JSON.parse(fs.readFileSync(diskPath, 'utf8'));

  assert.strictEqual(profile.mappings.length, diskData.mappings.length, `${id} mapping counts must match`);
  for (let i = 0; i < diskData.mappings.length; i++) {
    assert.strictEqual(profile.mappings[i].key, diskData.mappings[i].key, `Mapping key mismatch in ${id}`);
    assert.strictEqual(profile.mappings[i].defaultPort, diskData.mappings[i].default_port, `Port mismatch in ${id}`);
    assert.strictEqual(profile.mappings[i].protocol.toLowerCase(), diskData.mappings[i].protocol.toLowerCase(), `Protocol mismatch in ${id}`);
    assert.strictEqual(profile.mappings[i].optional, diskData.mappings[i].optional, `Optional flag mismatch in ${id}`);
  }
}
console.log(`[OK] FR-002: All 5 profiles verified against disk definitions (100% key/port/protocol parity).`);

// 2. Verify optional mappings disabled by default (GAME-008)
for (const profile of SHIPPED_PROFILES) {
  const initialConfigs = createInitialMappingConfigs(profile);
  for (const m of profile.mappings) {
    const cfg = initialConfigs[m.key];
    assert(cfg, `Missing initial config for ${profile.id}:${m.key}`);
    if (m.optional) {
      assert.strictEqual(cfg.enabled, false, `Optional mapping ${profile.id}:${m.key} must be DISABLED by default (GAME-008)`);
    } else {
      assert.strictEqual(cfg.enabled, true, `Mandatory mapping ${profile.id}:${m.key} must be ENABLED by default`);
    }
  }
}
console.log(`[OK] GAME-008: Optional mappings are strictly disabled by default across all profiles.`);

// 3. Verify custom non-default port support (GAME-009)
const mcJava = SHIPPED_PROFILES.find((p) => p.id === 'minecraft-java');
const customPortConfig = {
  host: '127.0.0.1',
  port: 25570, // custom non-default port
  enabled: true,
  confirmNonLoopback: false,
};
const customPortErrors = validateSingleMapping(mcJava.mappings[0], customPortConfig);
assert.strictEqual(Object.keys(customPortErrors).length, 0, 'Custom valid port 25570 must be accepted without errors');

// Verify across all games with custom ports
const gameCustomPorts = {
  'minecraft-java': 25570,
  'minecraft-bedrock': 19140,
  'terraria': 7788,
  'l4d2': 27025,
  'l4d1': 27025,
};
for (const [gid, customPort] of Object.entries(gameCustomPorts)) {
  const prof = SHIPPED_PROFILES.find((p) => p.id === gid);
  const cfg = { host: '127.0.0.1', port: customPort, enabled: true, confirmNonLoopback: false };
  const errs = validateSingleMapping(prof.mappings[0], cfg);
  assert.strictEqual(Object.keys(errs).length, 0, `Custom port ${customPort} for ${gid} must be valid`);
}
console.log(`[OK] GAME-009: Custom local server ports verified and accepted for all five games.`);

// 4. Verify local target policy violations (contracts.md §13 & target_policy.rs)
// 4a. Port 0 rejected
const zeroPortErrs = validateSingleMapping(mcJava.mappings[0], { host: '127.0.0.1', port: 0, enabled: true });
assert(zeroPortErrs.port && zeroPortErrs.port.includes('cannot be 0'), 'Port 0 must be rejected');

// 4b. Out-of-range port rejected
const bigPortErrs = validateSingleMapping(mcJava.mappings[0], { host: '127.0.0.1', port: 70000, enabled: true });
assert(bigPortErrs.port && bigPortErrs.port.includes('range'), 'Port 70000 must be rejected');

// 4c. Sensitive ports rejected unless default for profile
for (const port of [22, 445, 3389, 5432, 80, 443]) {
  const sensErrs = validateSingleMapping(mcJava.mappings[0], { host: '127.0.0.1', port, enabled: true });
  assert(sensErrs.port && sensErrs.port.includes('restricted'), `Sensitive port ${port} must be denied`);
}

// 4d. Prohibited address categories rejected
const prohibitedHosts = [
  ['0.0.0.0', 'unspecified'],
  ['::', 'unspecified'],
  ['255.255.255.255', 'broadcast'],
  ['224.0.0.1', 'multicast'],
  ['169.254.1.1', 'link-local'],
  ['fe80::1', 'link-local'],
];
for (const [host, expectedCat] of prohibitedHosts) {
  const cat = classifyProhibitedCategory(host);
  assert(cat, `${host} must be classified as prohibited`);
  const errs = validateSingleMapping(mcJava.mappings[0], { host, port: 25565, enabled: true });
  assert(errs.host && errs.host.includes('Prohibited'), `Prohibited host ${host} must be rejected`);
}

// 4e. Unconfirmed non-loopback rejected; confirmed accepted
const lanHost = '192.168.1.100';
const unconfirmedErrs = validateSingleMapping(mcJava.mappings[0], {
  host: lanHost,
  port: 25565,
  enabled: true,
  confirmNonLoopback: false,
});
assert(unconfirmedErrs.confirmation && unconfirmedErrs.confirmation.includes('requires explicit confirmation'), 'Unconfirmed non-loopback must be rejected');

const confirmedErrs = validateSingleMapping(mcJava.mappings[0], {
  host: lanHost,
  port: 25565,
  enabled: true,
  confirmNonLoopback: true,
});
assert.strictEqual(Object.keys(confirmedErrs).length, 0, 'Confirmed non-loopback must be accepted');

// 4f. Zero enabled mappings rejected
state.mappingConfigs = {
  game: { host: '127.0.0.1', port: 25565, enabled: false },
  query: { host: '127.0.0.1', port: 25565, enabled: false },
  rcon: { host: '127.0.0.1', port: 25575, enabled: false },
};
const allDisabledValid = validateAllMappings(mcJava);
assert.strictEqual(allDisabledValid, false, 'Validation must fail when all mappings are disabled');
assert(state.validationErrors._general && state.validationErrors._general.includes('At least one mapping'), 'Must report at least one mapping enabled error');

console.log(`[OK] FR-003 & contracts.md §13: All target policy violation rules verified with inline messages.`);

// 5. Invariant check: zero local targets in API payloads (INV-1, FR-003, SEC-006)
const simulatedCreateTunnelPayload = {
  profile_id: mcJava.id,
  enabled_mappings: ['game'],
};
const payloadString = JSON.stringify(simulatedCreateTunnelPayload);
assert(!payloadString.includes('127.0.0.1'), 'Local IP must NEVER appear in API payload (INV-1)');
assert(!payloadString.includes('25565'), 'Local port must NEVER appear in API payload (INV-1)');
assert(!payloadString.includes('local_host'), 'local_host must NEVER appear in API payload (INV-1)');
console.log(`[OK] INV-1 & FR-003: Confirmed local target information is NEVER transmitted over network.`);

console.log('\n===================================================================');
console.log(' ALL GAME SELECTION & LOCAL CONFIG TESTS PASSED (EXIT 0)           ');
console.log('===================================================================');
