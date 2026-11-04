/**
 * Invitations, One-Time Secret Display, Guest Claiming & Revocation Test Suite
 * Tasks & Requirements: T-084, FR-007, FR-008, FR-009, SEC-007, contracts.md §5.11–§5.13 & §6
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

const DESKTOP_DIR = path.resolve(__dirname, '..');
const APP_JS_PATH = path.join(DESKTOP_DIR, 'src', 'app.js');
const GUEST_HTML_PATH = path.join(DESKTOP_DIR, 'guest-claim.html');

console.log('Testing Invitations, Sharing & Guest Claim Page (T-084)...');

// 1. Verify standalone Guest Claim Page G1 exists and has required structure
assert(fs.existsSync(GUEST_HTML_PATH), 'guest-claim.html (G1) must exist');
const guestHtml = fs.readFileSync(GUEST_HTML_PATH, 'utf8');
assert(guestHtml.includes('src/tokens.css'), 'guest-claim.html must load tokens.css');
assert(guestHtml.includes('src/app.css'), 'guest-claim.html must load app.css');
assert(guestHtml.includes('guest-secret-input'), 'guest-claim.html must include secret input');
assert(guestHtml.includes('Same-network requirement'), 'guest-claim.html must include network mismatch tip (user-flows.md §7)');
console.log('[OK] G1 standalone guest claim page (apps/desktop/guest-claim.html) verified.');

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
  createInvitation,
  listInvitations,
  dismissOneTimeSecret,
  revokeInvitation,
  revokeAdmission,
  claimInvitation,
  renderInvitationsView,
  setTunnelState,
  state,
} = sandbox;

// 2. Verify negative validation on createInvitation (TEST-001, contracts.md §5.11)
const invalidExpiryLow = createInvitation({ expiresInSeconds: 10, maxClaims: 4 });
assert.strictEqual(invalidExpiryLow.ok, false, 'Expiry < 60s must be rejected');
assert.strictEqual(invalidExpiryLow.error.code, 'validation_failed');

const invalidClaimsHigh = createInvitation({ expiresInSeconds: 3600, maxClaims: 100 });
assert.strictEqual(invalidClaimsHigh.ok, false, 'maxClaims > 32 must be rejected');
assert.strictEqual(invalidClaimsHigh.error.code, 'validation_failed');

setTunnelState('Failed');
const conflictOnFailed = createInvitation({ expiresInSeconds: 3600, maxClaims: 4 });
assert.strictEqual(conflictOnFailed.ok, false, 'Creating invitation when tunnel is Failed must return conflict');
assert.strictEqual(conflictOnFailed.error.code, 'conflict');
setTunnelState('Connected');
console.log('[OK] Negative input & state conflict checks on createInvitation verified.');

// 3. Verify One-Time Secret Display & Non-Persistence in List (FR-007, SEC-007, contracts.md §5.11–§5.12)
const created = createInvitation({ expiresInSeconds: 3600, maxClaims: 2 });
assert.strictEqual(created.ok, true, 'Valid createInvitation must succeed');
assert(created.oneTimeSecret.startsWith('gtt_inv_'), 'Secret must use gtt_inv_ prefix');
// 24 base64url chars after prefix = 24 * 6 = 144 bits of entropy (>= 128 bits per SEC-007)
const rawPayload = created.oneTimeSecret.slice('gtt_inv_'.length);
assert(rawPayload.length >= 22, 'Secret must have >= 128 bits of entropy (SEC-007)');

// Secret must NOT exist on the returned invitation list item or in listInvitations()
assert.strictEqual(created.invitation.secret, undefined, 'Invitation record must not include secret field');
const listed = listInvitations();
for (const item of listed) {
  assert.strictEqual(item.secret, undefined, 'listInvitations() must never include secrets (FR-007)');
  assert(!JSON.stringify(item).includes(created.oneTimeSecret), 'Serialized list item must never contain secret');
}

// Immediately after creation, renderInvitationsView() shows the one-time banner with the secret
let viewHtml = renderInvitationsView();
assert(viewHtml.includes('id="one-time-secret-banner"'), 'One-time secret banner must appear right after creation');
assert(viewHtml.includes(created.oneTimeSecret), 'One-time secret must be visible once in the creation banner');

// Once dismissed, the secret is NEVER shown again
dismissOneTimeSecret();
viewHtml = renderInvitationsView();
assert(!viewHtml.includes('id="one-time-secret-banner"'), 'One-time secret banner must disappear after dismissal');
assert(!viewHtml.includes(created.oneTimeSecret), 'Secret must NEVER appear in rendered HTML after dismissal (FR-007)');
console.log('[OK] FR-007 & SEC-007: Secret has >= 128-bit entropy, is shown once, and never appears in list responses.');

// 4. Verify Guest Claiming & Exhaustion (FR-008, contracts.md §6)
const claim1 = claimInvitation(created.oneTimeSecret, '203.0.113.0/24');
assert.strictEqual(claim1.ok, true, 'First guest claim within maxClaims=2 must succeed');
assert.strictEqual(claim1.data.public_address, state.publicAddress, 'Claim must return public relay address');
assert(claim1.data.mappings.length >= 1, 'Claim must return enabled mappings');
assert(claim1.data.instruction.includes('Minecraft'), 'Claim must include game-specific connect instruction');

const claim2 = claimInvitation(created.oneTimeSecret, '198.51.100.0/24');
assert.strictEqual(claim2.ok, true, 'Second guest claim (reaching maxClaims=2) must succeed');

// Third claim on maxClaims=2 must fail with uniform "invitation_invalid"
const claim3Exhausted = claimInvitation(created.oneTimeSecret, '192.0.2.0/24');
assert.strictEqual(claim3Exhausted.ok, false, 'Claim beyond maxClaims must be rejected');
assert.strictEqual(claim3Exhausted.error.code, 'invitation_invalid', 'Exhausted secret must return invitation_invalid');

// Unknown secret must return identical "invitation_invalid" code and message
const claimUnknown = claimInvitation('gtt_inv_nonexistent_secret_token_999');
assert.strictEqual(claimUnknown.ok, false);
assert.strictEqual(claimUnknown.error.code, 'invitation_invalid');
assert.strictEqual(claimUnknown.error.message, claim3Exhausted.error.message, 'Error message must be identical so a guesser learns nothing');
console.log('[OK] FR-008 & contracts.md §6: Guest claiming, exhaustion, and uniform invitation_invalid verified.');

// 5. Verify Invitation Revocation & Cascading Admission Revocation (FR-009, contracts.md §5.12–§5.13)
const revocable = createInvitation({ expiresInSeconds: 7200, maxClaims: 4 });
const revSecret = revocable.oneTimeSecret;
const revInvId = revocable.invitation.id;

// Claim once before revoking so an active admission exists for this invitation
const preRevokeClaim = claimInvitation(revSecret, '100.64.12.0/24');
assert.strictEqual(preRevokeClaim.ok, true);
const createdAdmissionId = preRevokeClaim.data.admission_id;

// Revoke the invitation
const revokeResult = revokeInvitation(revInvId);
assert.strictEqual(revokeResult.ok, true, 'revokeInvitation must succeed');
assert.strictEqual(revokeResult.invitation.status, 'revoked', 'Invitation status must become "revoked"');

// Verify revocation is reflected in listInvitations() and rendered HTML table
const postRevokeList = listInvitations();
const revokedItem = postRevokeList.find((i) => i.id === revInvId);
assert(revokedItem, 'Revoked invitation must remain visible in list with revoked status');
assert.strictEqual(revokedItem.status, 'revoked', 'Revocation must be reflected in list (T-084 Done-when)');

viewHtml = renderInvitationsView();
assert(
  viewHtml.includes(`data-invitation-id="${revInvId}" data-status="revoked"`),
  'Rendered invitations table must reflect data-status="revoked"'
);

// Verify linked admission was automatically revoked (cascading revocation per contracts.md §5.12)
const cascadedAdm = state.guestAdmissions.find((a) => a.id === createdAdmissionId);
assert(cascadedAdm, 'Admission record must exist');
assert.strictEqual(cascadedAdm.status, 'revoked', 'Revoking invitation must cascade revocation to its admissions');

// Verify claiming a revoked invitation secret returns invitation_invalid
const claimAfterRevoke = claimInvitation(revSecret, '100.64.99.0/24');
assert.strictEqual(claimAfterRevoke.ok, false, 'Claiming a revoked invitation must fail');
assert.strictEqual(claimAfterRevoke.error.code, 'invitation_invalid');

// Verify direct single-admission revocation
const activeAdm = state.guestAdmissions.find((a) => a.status === 'active');
assert(activeAdm, 'Should have at least one active admission to test single revocation');
const admRevokeRes = revokeAdmission(activeAdm.id);
assert.strictEqual(admRevokeRes.ok, true);
assert.strictEqual(admRevokeRes.admission.status, 'revoked');
console.log('[OK] FR-009: Invitation and admission revocation reflected in list and enforced on claim.');

console.log('\n===================================================================');
console.log(' ALL INVITATION & SHARING TESTS PASSED (EXIT 0)                    ');
console.log('===================================================================');
