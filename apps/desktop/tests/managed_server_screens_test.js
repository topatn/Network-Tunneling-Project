/**
 * Managed Server Screens & 4th Status Indicator Test Suite
 * Tasks & Requirements: T-088, SRV-001, SRV-002, SRV-003, SRV-004, SRV-006, SRV-009, SRV-011
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

const APP_JS_PATH = path.resolve(__dirname, '..', 'src', 'app.js');

console.log('Testing Managed Server Screens & 4th Independent Status Indicator (T-088)...');

assert(fs.existsSync(APP_JS_PATH), 'app.js must exist');
const appJsCode = fs.readFileSync(APP_JS_PATH, 'utf8');

const mainContentMock = { innerHTML: '' };
const windowMock = {
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
  SHIPPED_PROFILES,
  PROVISIONABLE_PROFILE_IDS,
  MANAGED_SERVER_STATE_SPEC,
  MANAGED_SERVER_STATE_MESSAGES,
  MANAGED_SERVER_FAILURE_CODES,
  ERROR_MESSAGE_CATALOG,
  createInitialMappingConfigs,
  isProfileProvisionable,
  setTunnelState,
  setLocalServerState,
  setPlayersSeen,
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
  renderLocalConfigView,
  renderDashboardView,
  renderManagedServerView,
  state,
} = sandbox;

// Helper to extract indicator attributes from rendered HTML
function extractIndicatorAttributes(html, indicatorId) {
  const regex = new RegExp(
    `<div class="kpi" id="${indicatorId}"\\s+data-indicator="([^"]+)"\\s+data-state="([^"]+)"\\s+data-token="([^"]+)"\\s+aria-live="([^"]+)"`,
    'i'
  );
  const match = regex.exec(html);
  assert(match, `Indicator #${indicatorId} must be present in rendered HTML`);
  return {
    indicator: match[1],
    state: match[2],
    token: match[3],
    ariaLive: match[4],
  };
}

// 1. Verify Provisionable Allow-List & Manual Configuration Parity for All 5 Profiles (SRV-003, user-flows.md §5 Journey 5b)
assert.strictEqual(PROVISIONABLE_PROFILE_IDS.size, 1);
assert(isProfileProvisionable('minecraft-java'), 'minecraft-java must be provisionable (SRV-003)');

const nonProvisionableIds = ['minecraft-bedrock', 'terraria', 'l4d2', 'l4d1'];
for (const pid of nonProvisionableIds) {
  assert(!isProfileProvisionable(pid), `${pid} must NOT be on the provisionable list in the first cut (SRV-003, ADR-013)`);
}

for (const profile of SHIPPED_PROFILES) {
  state.selectedProfile = profile;
  state.mappingConfigs = createInitialMappingConfigs(profile);
  const localConfigHtml = renderLocalConfigView();

  // Manual configuration form must always be rendered and supported for all 5 games
  assert(
    localConfigHtml.includes('id="local-config-form"'),
    `Manual configuration form must exist on S5 for ${profile.id}`
  );
  assert(
    localConfigHtml.includes(`id="port-${profile.mappings[0].key}"`),
    `Primary port input must exist on S5 for ${profile.id}`
  );

  if (profile.id === 'minecraft-java') {
    assert(
      localConfigHtml.includes('id="btn-setup-managed-server"') &&
        localConfigHtml.includes('Set this up for me'),
      'S5 must offer "Set this up for me" for minecraft-java (SRV-003)'
    );
    assert(
      !localConfigHtml.includes('id="manual-only-provisioning-note"'),
      'minecraft-java must not show manual-only notice'
    );
  } else {
    assert(
      !localConfigHtml.includes('id="btn-setup-managed-server"'),
      `S5 must NOT show "Set this up for me" button (not even disabled) for non-provisionable profile ${profile.id} (user-flows.md §5 Journey 5b)`
    );
    assert(
      localConfigHtml.includes('id="manual-only-provisioning-note"'),
      `S5 must state plainly that manual configuration is the only path for ${profile.id}`
    );
  }
}
// Restore Minecraft: Java Edition as selected profile
state.selectedProfile = SHIPPED_PROFILES[0];
state.mappingConfigs = createInitialMappingConfigs(SHIPPED_PROFILES[0]);
console.log('[OK] SRV-003: "Set this up for me" offered only for minecraft-java; manual configuration works across all 5 profiles.');

// 2. Verify Every Managed Server State & Failure Code Has a Reviewed Cause and Action (T-088 Done-when)
const allManagedStates = [
  'Not set up',
  'Downloading',
  'Verifying',
  'Configuring',
  'Starting',
  'Running',
  'Crashed',
  'Updating',
  'Failed',
];

for (const st of allManagedStates) {
  assert(MANAGED_SERVER_STATE_SPEC[st], `MANAGED_SERVER_STATE_SPEC must define "${st}"`);
  const msg = MANAGED_SERVER_STATE_MESSAGES[st];
  assert(msg, `MANAGED_SERVER_STATE_MESSAGES must define reviewed copy for "${st}"`);
  assert(msg.headline && msg.headline.length >= 15, `State "${st}" must have a clear headline`);
  assert(msg.cause && msg.cause.length >= 20, `State "${st}" must state a cause`);
  assert(msg.action && msg.action.length >= 20, `State "${st}" must state a next action`);
}

for (const errCode of MANAGED_SERVER_FAILURE_CODES) {
  const entry = ERROR_MESSAGE_CATALOG[errCode];
  assert(entry, `ERROR_MESSAGE_CATALOG must include managed server failure code ${errCode}`);
  assert.strictEqual(entry.category, 'engine');
  assert(entry.title && entry.title.length >= 10, `${errCode} must have a reviewed title`);
  assert(entry.cause && entry.cause.length >= 20, `${errCode} must have a reviewed cause`);
  assert(entry.action && entry.action.length >= 20, `${errCode} must have a reviewed next action`);
}
console.log(`[OK] T-088: All ${allManagedStates.length} managed server states and ${MANAGED_SERVER_FAILURE_CODES.length} failure codes have reviewed cause-and-action messages.`);

// 3. Drive All 9 Managed Server States & Verify 4th Indicator Independence (SRV-006, FR-010, UX-002)
setTunnelState('Connected');
setLocalServerState('Not reachable');
setPlayersSeen(2);

for (const targetManagedState of allManagedStates) {
  setManagedServerState(targetManagedState);
  const dashHtml = renderDashboardView();
  const s12Html = renderManagedServerView();

  const tunnelAttr = extractIndicatorAttributes(dashHtml, 'indicator-tunnel');
  const localAttr = extractIndicatorAttributes(dashHtml, 'indicator-local-server');
  const playersAttr = extractIndicatorAttributes(dashHtml, 'indicator-players-seen');
  const managedDashAttr = extractIndicatorAttributes(dashHtml, 'indicator-managed-server');
  const managedS12Attr = extractIndicatorAttributes(s12Html, 'indicator-managed-server');

  assert.strictEqual(managedDashAttr.state, targetManagedState);
  assert.strictEqual(managedDashAttr.token, MANAGED_SERVER_STATE_SPEC[targetManagedState].token);
  assert.strictEqual(managedS12Attr.state, targetManagedState);

  // SRV-006: Even when Managed Server is "Running", Local Server MUST still read "Not reachable" from the L0 probe!
  assert.strictEqual(tunnelAttr.state, 'Connected', `Tunnel state must remain Connected when managed server is ${targetManagedState}`);
  assert.strictEqual(localAttr.state, 'Not reachable', `Local server indicator must remain Not reachable (never overridden by managed server ${targetManagedState}, SRV-006)`);
  assert.strictEqual(playersAttr.state, '2 guests seen', `Players seen must remain 2 guests seen when managed server is ${targetManagedState}`);
}
console.log('[OK] SRV-006: Managed server 4th indicator transitions across all 9 states without affecting Tunnel, Local Server, or Players Seen.');

// 4. Verify Host EULA Gate, Multi-Step Provisioning, Honest Cancel, and SHA-256 Verification Failure (SRV-001, SRV-002)
setManagedServerState('Not set up');
acceptManagedServerTerms(false);

// 4a: Attempting to provision without EULA confirmation fails with srv_terms_not_accepted
const refusedStart = startManagedServerProvisioning();
assert.strictEqual(refusedStart.ok, false, 'Provisioning must be refused until host confirms EULA terms (SRV-001)');
assert.strictEqual(refusedStart.error.code, 'srv_terms_not_accepted');
assert.strictEqual(state.managedServer.state, 'Failed');

// 4b: Host confirms EULA -> provisioning progresses through Downloading -> Verifying -> Cancel
acceptManagedServerTerms(true);
const started = startManagedServerProvisioning();
assert.strictEqual(started.ok, true);
assert.strictEqual(state.managedServer.state, 'Downloading');
assert.strictEqual(state.managedServer.stepStatuses.download, 'active');

advanceManagedServerProvisioningStep();
assert.strictEqual(state.managedServer.state, 'Verifying');
assert.strictEqual(state.managedServer.stepStatuses.download, 'completed');
assert.strictEqual(state.managedServer.stepStatuses.verify, 'active');

// Cancel during Verifying -> returns to Not set up and records honest summary of what happened
const cancelMsg = cancelManagedServerProvisioning();
assert.strictEqual(state.managedServer.state, 'Not set up');
assert(
  cancelMsg.includes('Cancelled during Verifying') && cancelMsg.includes('no server process was started'),
  'Cancel must state honestly what step was active and that temporary files were removed (DESIGN.md §8)'
);
assert(
  renderManagedServerView().includes('id="managed-cancel-summary"'),
  'S12 must render the honest cancellation summary banner'
);

// 4c: Verification failure (srv_verification_failed) is terminal, discards artifact, and never runs it (SRV-002)
startManagedServerProvisioning();
advanceManagedServerProvisioningStep(); // Verifying
setManagedServerState('Failed', { failureCode: 'srv_verification_failed' });
assert.strictEqual(state.managedServer.artifactVerified, false);
assert.strictEqual(state.managedServer.artifactDiscarded, true, 'Unverified artifact must be discarded on SHA-256 mismatch (SRV-002)');
const failedVerifyHtml = renderManagedServerView();
assert(
  failedVerifyHtml.includes('The unverified download artifact was discarded and never executed'),
  'S12 must explicitly confirm the unverified artifact was discarded and never run'
);

// 4d: Complete full provisioning sequence to Running -> Stop without editing files by hand
startManagedServerProvisioning(); // Downloading
advanceManagedServerProvisioningStep(); // Verifying
advanceManagedServerProvisioningStep(); // Configuring
assert.strictEqual(state.managedServer.state, 'Configuring');
advanceManagedServerProvisioningStep(); // Starting
assert.strictEqual(state.managedServer.state, 'Starting');
advanceManagedServerProvisioningStep(); // Running
assert.strictEqual(state.managedServer.state, 'Running');
assert.strictEqual(state.managedServer.progressPercent, 100);
console.log('[OK] SRV-001 & SRV-002: EULA gate, discrete 4-step provisioning, honest cancel, and SHA-256 verification enforcement verified.');

// 5. Verify Crash Excerpt Behind Details, World Backup/Restore, and Active-Tunnel Update Confirmation (SRV-009)
setManagedServerState('Crashed', { failureCode: 'srv_process_crashed', restartAttempt: 2 });
const crashedHtml = renderManagedServerView();
assert(
  crashedHtml.includes('id="managed-crash-details"') && crashedHtml.includes('OutOfMemoryError'),
  'Crashed state must expose the server crash log excerpt behind <details> (DESIGN.md §6)'
);
assert(
  crashedHtml.includes('Restart attempt 2 / 3'),
  'Crashed state must show bounded restart attempt counter'
);

// Restore Running state and test World Backup & Restore
setManagedServerState('Running');
const initialBackupCount = state.managedServer.backups.length;
const newBackup = createManagedWorldBackup('Before Nether expansion');
assert.strictEqual(state.managedServer.backups.length, initialBackupCount + 1);
const restoreRes = restoreManagedWorldBackup(newBackup.id);
assert.strictEqual(restoreRes.ok, true);
assert.strictEqual(state.managedServer.lastRestoredBackupId, newBackup.id);

// Test Update while Tunnel is Connected -> MUST open confirmation modal and wait (SRV-009, DESIGN.md §7)
setTunnelState('Connected');
const updateWhileActive = requestManagedServerUpdate('1.21.3');
assert.strictEqual(updateWhileActive.updatedImmediately, false, 'Must NOT update immediately while tunnel is Connected (SRV-009)');
assert.strictEqual(updateWhileActive.modalOpened, true, 'Must open confirmation modal before updating when tunnel is active');
assert.strictEqual(state.isManagedUpdateModalOpen, true);
assert.strictEqual(state.managedServer.currentVersion, '1.21.1', 'Server version must remain unchanged while awaiting confirmation');

// Cancel update modal -> stays on 1.21.1
cancelManagedServerUpdate();
assert.strictEqual(state.isManagedUpdateModalOpen, false);
assert.strictEqual(state.managedServer.currentVersion, '1.21.1');

// Confirm update modal -> creates automatic pre-update backup and updates to 1.21.3
requestManagedServerUpdate('1.21.3');
const backupsBeforeConfirm = state.managedServer.backups.length;
confirmManagedServerUpdate();
assert.strictEqual(state.isManagedUpdateModalOpen, false);
assert.strictEqual(state.managedServer.currentVersion, '1.21.3', 'Server version must update to 1.21.3 after confirmation');
assert.strictEqual(state.managedServer.backups.length, backupsBeforeConfirm + 1, 'Update must automatically create a pre-update world backup (SRV-009)');

// Test Update while Tunnel is Not running -> updates immediately without confirmation modal
setTunnelState('Not running');
const updateWhileStopped = requestManagedServerUpdate('1.21.4');
assert.strictEqual(updateWhileStopped.updatedImmediately, true, 'Update when tunnel is stopped must proceed without modal');
assert.strictEqual(updateWhileStopped.modalOpened, false);
assert.strictEqual(state.managedServer.currentVersion, '1.21.4');

// Stop managed server cleanly
stopManagedServer();
assert.strictEqual(state.managedServer.state, 'Configuring');
assert.strictEqual(state.managedServer.termsAccepted, true);
console.log('[OK] SRV-009: Crash log details, world backup/restore, and active-tunnel update confirmation gate verified.');

// 6. Verify Opt-in Confirmed In-Game Presence Never Merges with Observed Traffic Count (SRV-011, UX-002)
setPlayersSeen(4);
setConfirmedInGamePresence(2);
const presenceDashHtml = renderDashboardView();
const playersSeenAttr = extractIndicatorAttributes(presenceDashHtml, 'indicator-players-seen');
const confirmedAttr = extractIndicatorAttributes(presenceDashHtml, 'indicator-confirmed-presence');

assert.strictEqual(playersSeenAttr.state, '4 guests seen', 'Observed traffic count must remain "4 guests seen"');
assert.strictEqual(
  confirmedAttr.state,
  '2 in game (reported by the server)',
  'Opt-in confirmed presence must render separately as "2 in game (reported by the server)" (SRV-011)'
);

setConfirmedInGamePresence(null);
const fallbackDashHtml = renderDashboardView();
const fallbackConfirmedAttr = extractIndicatorAttributes(fallbackDashHtml, 'indicator-confirmed-presence');
assert(
  fallbackConfirmedAttr.state.includes('traffic count only'),
  'When Server Management Protocol is off, UI must fall back to traffic count only and say so (user-flows.md §0)'
);
console.log('[OK] SRV-011 & UX-002: Opt-in confirmed in-game presence is strictly separate from observed traffic count.');

console.log('\n===================================================================');
console.log(' ALL MANAGED SERVER SCREEN TESTS PASSED (EXIT 0)                   ');
console.log('===================================================================');
