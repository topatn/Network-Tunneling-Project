// Data Persistence & Recovery Test Suite (T-080, T-082, T-084, T-088)
const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

const appJsPath = path.join(__dirname, '../src/app.js');
const appJsCode = fs.readFileSync(appJsPath, 'utf8');

console.log('Testing GTT Desktop Data Persistence & State Recovery...');

// Mock localStorage with in-memory store
const storageMap = new Map();
const localStorageMock = {
  getItem: (key) => storageMap.get(key) || null,
  setItem: (key, val) => storageMap.set(key, String(val)),
  removeItem: (key) => storageMap.delete(key),
  clear: () => storageMap.clear(),
};

const mainContentMock = {
  innerHTML: '',
  className: '',
  style: {},
  querySelector: () => null,
  querySelectorAll: () => [],
};

const windowMock = {
  location: { hash: '' },
  addEventListener: () => {},
  document: {
    addEventListener: () => {},
    getElementById: (id) => (id === 'main-content' ? mainContentMock : null),
    querySelectorAll: () => [],
    querySelector: () => null,
    createElement: () => ({
      className: '',
      setAttribute: () => {},
      textContent: '',
      remove: () => {},
      appendChild: () => {},
    }),
    documentElement: { setAttribute: () => {}, removeAttribute: () => {} },
  },
  localStorage: localStorageMock,
};

const sandbox = {
  window: windowMock,
  document: windowMock.document,
  localStorage: localStorageMock,
  console,
  setTimeout,
  clearTimeout,
};

vm.createContext(sandbox);
vm.runInContext(appJsCode, sandbox);

const {
  state,
  savePersistedState,
  loadPersistedState,
  syncBackendState,
  SHIPPED_PROFILES,
  acceptManagedServerTerms,
  createManagedWorldBackup,
  createInvitation,
  navigateTo,
  stopManagedServer,
} = sandbox;

assert.strictEqual(typeof savePersistedState, 'function', 'savePersistedState must be defined');
assert.strictEqual(typeof loadPersistedState, 'function', 'loadPersistedState must be defined');

// 1. Test selected profile persistence
console.log('Testing Profile & Custom Mapping Persistence...');
const terraria = SHIPPED_PROFILES.find((p) => p.id === 'terraria');
assert.ok(terraria, 'Terraria profile should exist');
state.selectedProfile = terraria;
state.mappingConfigs = {
  game: { host: '127.0.0.1', port: 7778, enabled: true, confirmNonLoopback: false },
};
navigateTo('local-config');
savePersistedState();

assert.strictEqual(localStorageMock.getItem('gtt-selected-profile-id'), 'terraria');
assert.strictEqual(localStorageMock.getItem('gtt-current-view'), 'local-config');

// Reset in-memory state and load from persistence
state.selectedProfile = SHIPPED_PROFILES[0];
state.mappingConfigs = {};
loadPersistedState();

assert.strictEqual(state.selectedProfile.id, 'terraria', 'Persisted profile should be restored');
assert.strictEqual(state.mappingConfigs.game.port, 7778, 'Persisted custom port should be restored');
assert.strictEqual(state.currentView, 'local-config', 'Persisted active view should be restored');
console.log('[OK] Profile, mapping config, and active view persistence verified.');

// 2. Test Managed Server state & EULA acceptance persistence
console.log('Testing Managed Server State & EULA Persistence...');
acceptManagedServerTerms(true);
state.managedServer.jarPath = 'C:\\Games\\server.jar';
state.managedServer.state = 'Configuring';
state.managedServer.progressPercent = 75;
createManagedWorldBackup('Test Persistent Snapshot');
savePersistedState();

assert.strictEqual(localStorageMock.getItem('gtt-ms-terms-accepted'), 'true');
assert.strictEqual(localStorageMock.getItem('gtt-ms-jar-path'), 'C:\\Games\\server.jar');

// Reset managedServer in-memory state and load
state.managedServer.termsAccepted = false;
state.managedServer.jarPath = '';
state.managedServer.state = 'Not set up';
state.managedServer.backups = [];
loadPersistedState();

assert.strictEqual(state.managedServer.termsAccepted, true, 'EULA terms acceptance must persist');
assert.strictEqual(state.managedServer.jarPath, 'C:\\Games\\server.jar', 'JAR path must persist');
assert.strictEqual(state.managedServer.state, 'Configuring', 'Configuring state must persist');
assert.ok(state.managedServer.backups.length > 0, 'Backups list must persist');
assert.strictEqual(state.managedServer.backups[0].label, 'Test Persistent Snapshot');
console.log('[OK] Managed Server EULA, JAR path, state, and backups persistence verified.');

// 3. Test stopManagedServer preserves configured provisioned state on disk
console.log('Testing stopManagedServer preserves configured provisioned state...');
state.managedServer.state = 'Running';
state.managedServer.stepStatuses = {
  download: 'completed',
  verify: 'completed',
  configure: 'completed',
  start: 'completed',
};
state.managedServer.progressPercent = 100;
state.managedServer.termsAccepted = true;

stopManagedServer();

assert.strictEqual(
  state.managedServer.state,
  'Configuring',
  'Stopping a provisioned server must transition to Configuring (ready to start), not Not set up'
);
assert.strictEqual(state.managedServer.termsAccepted, true, 'EULA acceptance must be preserved on stop');
assert.strictEqual(
  state.managedServer.stepStatuses.configure,
  'completed',
  'Configuration step must remain completed on stop'
);
console.log('[OK] stopManagedServer persistence of configured state verified.');

// 4. Test Invitations persistence
console.log('Testing Invitations List Persistence...');
state.tunnelState = 'Connected';
createInvitation({ expiresInSeconds: 7200, maxClaims: 8 });
savePersistedState();

assert.ok(localStorageMock.getItem('gtt-invitations'), 'Invitations JSON must be stored');

// Reset invitations and load
state.invitations = [];
loadPersistedState();

assert.ok(state.invitations.length > 0, 'Invitations must be restored from localStorage');
assert.strictEqual(state.invitations[0].maxClaims, 8);
console.log('[OK] Invitations list persistence verified.');

console.log('\n===================================================================');
console.log(' ALL DATA PERSISTENCE & STATE RECOVERY TESTS PASSED (EXIT 0)       ');
console.log('===================================================================\n');
