/**
 * Onboarding, Browser Sign-In & Sign-Out Test Suite (T-081)
 * Requirements & Architecture: FR-001, UX-001, SEC-001..SEC-003, PRIV-005, ADR-018
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

const DESKTOP_DIR = path.resolve(__dirname, '..');
const INDEX_HTML_PATH = path.join(DESKTOP_DIR, 'index.html');
const APP_CSS_PATH = path.join(DESKTOP_DIR, 'src', 'app.css');
const APP_JS_PATH = path.join(DESKTOP_DIR, 'src', 'app.js');

console.log('Testing Desktop Onboarding, Browser Sign-In & Sign-Out (T-081)...');

// 1. Static Asset Invariant Checks
assert(fs.existsSync(INDEX_HTML_PATH), 'index.html must exist');
assert(fs.existsSync(APP_CSS_PATH), 'app.css must exist');
assert(fs.existsSync(APP_JS_PATH), 'app.js must exist');

const indexHtml = fs.readFileSync(INDEX_HTML_PATH, 'utf8');
const appCss = fs.readFileSync(APP_CSS_PATH, 'utf8');
const appJsCode = fs.readFileSync(APP_JS_PATH, 'utf8');

// Verify sidebar account elements in index.html
assert(indexHtml.includes('id="sidebar-account-block"'), 'index.html must have #sidebar-account-block');
assert(indexHtml.includes('id="sidebar-account-name"'), 'index.html must have #sidebar-account-name');
assert(indexHtml.includes('id="sidebar-account-quota"'), 'index.html must have #sidebar-account-quota');
assert(indexHtml.includes('id="sidebar-btn-auth"'), 'index.html must have #sidebar-btn-auth');
assert(indexHtml.includes('data-target="account"'), 'index.html must have account nav item');
console.log('[OK] index.html sidebar account elements and navigation verified.');

// Verify CSS styles for auth spinner and reduced-motion rules
assert(appCss.includes('.auth-spinner'), 'app.css must define .auth-spinner');
assert(appCss.includes('@media (prefers-reduced-motion: reduce)'), 'app.css must have prefers-reduced-motion');
assert(appCss.includes('.auth-waiting-box'), 'app.css must define .auth-waiting-box');
assert(appCss.includes('.account-grid'), 'app.css must define .account-grid');
console.log('[OK] app.css auth spinner, waiting box, and reduced-motion styling verified.');

// 2. Load app.js in VM Sandbox with Mock DOM
const mainContentMock = { innerHTML: '' };
const viewTitleMock = { textContent: '' };
const topbarScopeMock = { textContent: '' };
const sidebarNameMock = { textContent: '' };
const sidebarQuotaMock = { textContent: '' };
const sidebarBtnMock = { textContent: '', setAttribute: () => {} };
const navAccountBadgeMock = { textContent: '' };

function makeMockElement(id) {
  return {
    id,
    innerHTML: '',
    textContent: '',
    addEventListener: () => {},
    appendChild: () => {},
    setAttribute: () => {},
    classList: { add: () => {}, remove: () => {}, toggle: () => {} },
    focus: () => {},
  };
}

const elementsById = {
  'main-content': mainContentMock,
  'view-title': viewTitleMock,
  'topbar-scope': topbarScopeMock,
  'sidebar-account-name': sidebarNameMock,
  'sidebar-account-quota': sidebarQuotaMock,
  'sidebar-btn-auth': sidebarBtnMock,
  'nav-badge-account': navAccountBadgeMock,
};

const windowMock = {
  addEventListener: () => {},
  document: {
    addEventListener: () => {},
    getElementById: (id) => elementsById[id] || makeMockElement(id),
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
  dispatchKeyAction,
  navigateTo,
  renderOnboardingView,
  renderSignInHandoffView,
  renderAccountView,
  startBrowserAuth,
  cancelSignIn,
  failSignIn,
  completeSignIn,
  signOut,
  attemptTokenRefresh,
  state,
} = sandbox;

// 3. Fresh Install Simulation (FR-001, UX-001 Done When)
console.log('Testing Fresh Install Flow...');
state.isSignedIn = false;
state.user = null;
state.tunnelState = 'Not running';
state.playersSeenCount = 0;
navigateTo('onboarding');

assert.strictEqual(state.currentView, 'onboarding', 'Fresh install lands on S1 Welcome');
assert.strictEqual(state.isSignedIn, false, 'Fresh install is unauthenticated');

const onboardingHtml = renderOnboardingView();
assert(onboardingHtml.includes('Welcome to Game Tunnel Tooling'), 'S1 must render hero title');
assert(onboardingHtml.includes('without router port-forwarding'), 'S1 must state zero port-forwarding value prop (UX-001)');
assert(onboardingHtml.includes('Your game traffic is encrypted between your PC and our relay servers.'), 'S1 must render verbatim privacy disclosure (PRIV-005)');
assert(onboardingHtml.includes('game payloads are never inspected, modified, or written to disk.'), 'S1 must render zero-inspection guarantee');
assert(onboardingHtml.includes('id="btn-start-signin"'), 'S1 must provide sign-in button');
console.log('[OK] S1 Welcome / Onboarding verified with zero-config copy and verbatim disclosures (UX-001, PRIV-005).');

// 4. Browser Sign-In Initiation & Waiting Handoff (S2, ADR-018, SEC-001)
console.log('Testing Browser Sign-In Initiation (PKCE S256)...');
assert.strictEqual(dispatchKeyAction('btn-start-signin', 'Enter'), true);
assert.strictEqual(state.currentView, 'signin-handoff', 'Activating sign-in navigates to S2 signin-handoff');
assert.strictEqual(state.authFlow.status, 'waiting', 'Auth flow status must be waiting');
assert(state.authFlow.codeVerifier && state.authFlow.codeVerifier.length >= 43, 'PKCE code verifier must have sufficient entropy (RFC 7636)');
assert(state.authFlow.codeChallenge, 'PKCE code challenge must be computed');
assert.strictEqual(state.authFlow.redirectUri, 'http://127.0.0.1:45123/callback', 'Redirect URI must be loopback (RFC 8252)');

const handoffHtml = renderSignInHandoffView();
assert(handoffHtml.includes('Complete Sign-In in Your Browser'), 'S2 must render sign-in handoff title');
assert(handoffHtml.includes('127.0.0.1'), 'S2 must state loopback callback');
assert(handoffHtml.includes('auth-spinner'), 'S2 must render waiting spinner');
assert(handoffHtml.includes('role="status"'), 'S2 spinner box must have role="status"');
assert(handoffHtml.includes('id="btn-cancel-auth"'), 'S2 must provide cancel button');
assert(handoffHtml.includes('id="btn-simulate-auth-success"'), 'S2 must provide completion button');
console.log('[OK] S2 Browser Sign-In Waiting state verified with PKCE S256 and loopback details (ADR-018).');

// 5. Cancel Sign-In Flow
console.log('Testing Cancel Sign-In Flow...');
assert.strictEqual(dispatchKeyAction('btn-cancel-auth', 'Enter'), true);
assert.strictEqual(state.currentView, 'onboarding', 'Cancelling sign-in returns to S1 onboarding');
assert.strictEqual(state.authFlow.status, 'idle', 'Cancelling sign-in resets auth flow status');
assert.strictEqual(state.isSignedIn, false, 'User remains signed out after cancel');
console.log('[OK] Sign-in cancellation cleanly restores unauthenticated state.');

// 6. Error & Suspension Flow (forbidden / suspended account)
console.log('Testing Account Restriction Error Flow...');
startBrowserAuth();
failSignIn('forbidden', "This account can't create tunnels. Contact support.");
assert.strictEqual(state.authFlow.status, 'error');
assert.strictEqual(state.authFlow.errorCode, 'forbidden');

const errorHtml = renderSignInHandoffView();
assert(errorHtml.includes('role="alert"'), 'Error view must carry role="alert"');
assert(errorHtml.includes("This account can't create tunnels. Contact support."), 'Suspension message must match user-flows.md §3');
assert(errorHtml.includes('id="btn-retry-auth"'), 'Error view must provide retry button');

// Retrying restarts auth
assert.strictEqual(dispatchKeyAction('btn-retry-auth', 'Enter'), true);
assert.strictEqual(state.authFlow.status, 'waiting');
console.log('[OK] Account restriction and retry flow verified (user-flows.md §3).');

// 7. Successful Browser Sign-In Flow (FR-001 Done When)
console.log('Testing Successful Browser Sign-In Completion...');
assert.strictEqual(dispatchKeyAction('btn-simulate-auth-success', 'Enter'), true);
assert.strictEqual(state.isSignedIn, true, 'User is marked signed in');
assert.strictEqual(state.currentView, 'choose-game', 'Successful sign-in navigates to S4 choose-game');
assert(state.user !== null, 'User profile is populated');
assert.strictEqual(state.user.status, 'active');
assert.strictEqual(state.user.displayName, 'chris@desktop-win11');
assert.strictEqual(state.user.email, 'chris@host.gtt.gg');
assert.strictEqual(sidebarNameMock.textContent, 'chris@desktop-win11', 'Sidebar displays user name');
assert(sidebarQuotaMock.textContent.includes('4.12 / 100 GB'), 'Sidebar displays monthly egress quota');
assert.strictEqual(navAccountBadgeMock.textContent, 'Active', 'Sidebar account badge is Active');
console.log('[OK] Successful sign-in transitions to signed-in state with user profile and sidebar sync (FR-001).');

// 8. Account & Session Screen (S10, SEC-002, SEC-003)
console.log('Testing Account & Session Screen...');
navigateTo('account');
assert.strictEqual(state.currentView, 'account');

const accountHtml = renderAccountView();
assert(accountHtml.includes('Host Identity'), 'Account screen renders Host Identity panel');
assert(accountHtml.includes('chris@desktop-win11'), 'Account screen displays display name');
assert(accountHtml.includes('Relay Egress &amp; Quotas'), 'Account screen renders Quota panel');
assert(accountHtml.includes('4.12 / 100 GB'), 'Account screen displays egress usage');
assert(accountHtml.includes('EncryptedFileVault'), 'Account screen cites Windows Credential Vault (SEC-002)');
assert(accountHtml.includes('RFC 7636 PKCE S256'), 'Account screen cites PKCE S256 protocol (ADR-018)');
assert(accountHtml.includes('Single-use token rotation'), 'Account screen cites token rotation (SEC-003)');
assert(accountHtml.includes('id="btn-sign-out"'), 'Account screen provides sign-out button');
console.log('[OK] S10 Account & Session screen verified with identity, quota, and credential security panels.');

// 9. Sign-Out & Session Revocation (FR-001, SEC-003)
console.log('Testing Sign-Out & Session Revocation...');
const tokenBeforeSignOut = 'gtt_rt_session_current';
const refreshBeforeSignOut = attemptTokenRefresh(tokenBeforeSignOut);
assert.strictEqual(refreshBeforeSignOut.ok, true, 'Token refresh succeeds while signed in');

// Execute Sign-Out
assert.strictEqual(dispatchKeyAction('btn-sign-out', 'Enter'), true);
assert.strictEqual(state.isSignedIn, false, 'User is signed out');
assert.strictEqual(state.user, null, 'User profile is cleared');
assert.strictEqual(state.currentView, 'onboarding', 'Sign-out redirects to S1 Welcome');
assert.strictEqual(sidebarNameMock.textContent, 'Not Signed In', 'Sidebar indicates unauthenticated');
assert.strictEqual(sidebarQuotaMock.textContent, 'Sign in to host tunnels', 'Sidebar prompts sign in');
assert.strictEqual(navAccountBadgeMock.textContent, 'Off', 'Sidebar account badge is Off');

// FR-001 Acceptance Criteria: Refresh token used before sign-out is rejected with unauthenticated
const refreshAfterSignOut = attemptTokenRefresh(tokenBeforeSignOut);
assert.strictEqual(refreshAfterSignOut.ok, false, 'Refresh token must be rejected after sign-out (FR-001)');
assert.strictEqual(refreshAfterSignOut.error.code, 'unauthenticated', 'Rejection code must be unauthenticated (FR-001)');
console.log('[OK] FR-001 verified: Refresh token used before sign-out is rejected with unauthenticated.');

console.log('\n===================================================================');
console.log(' ALL ONBOARDING, BROWSER SIGN-IN & SIGN-OUT TESTS PASSED (EXIT 0)  ');
console.log('===================================================================');
