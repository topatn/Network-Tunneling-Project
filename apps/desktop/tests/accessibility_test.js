/**
 * Accessibility Pass Test Suite (Keyboard Operation, Focus Visibility & Screen-Reader Semantics)
 * Tasks & Requirements: T-086, UX-004, DESIGN.md §§6, 7, 9
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');
const vm = require('vm');

const DESKTOP_DIR = path.resolve(__dirname, '..');
const INDEX_HTML_PATH = path.join(DESKTOP_DIR, 'index.html');
const APP_CSS_PATH = path.join(DESKTOP_DIR, 'src', 'app.css');
const APP_JS_PATH = path.join(DESKTOP_DIR, 'src', 'app.js');

console.log('Testing Desktop Accessibility: Keyboard Operation, Focus Visibility & Screen Reader Labels (T-086)...');

assert(fs.existsSync(INDEX_HTML_PATH), 'index.html must exist');
assert(fs.existsSync(APP_CSS_PATH), 'app.css must exist');
assert(fs.existsSync(APP_JS_PATH), 'app.js must exist');

const indexHtml = fs.readFileSync(INDEX_HTML_PATH, 'utf8');
const appCss = fs.readFileSync(APP_CSS_PATH, 'utf8');
const appJsCode = fs.readFileSync(APP_JS_PATH, 'utf8');

// 1. Verify Focus Visibility & Reduced-Motion Rules in app.css (DESIGN.md §9)
assert(
  appCss.includes('*:focus-visible') && appCss.includes('outline: 2px solid var(--gtt-color-focus)'),
  'app.css must define a visible 2px focus ring using --gtt-color-focus on *:focus-visible (DESIGN.md §9)'
);
assert(
  appCss.includes('outline-offset: 2px'),
  'app.css focus ring must have 2px outline-offset (DESIGN.md §9)'
);
assert(
  appCss.includes('.skip-link'),
  'app.css must style the keyboard skip-to-main link'
);
assert(
  appCss.includes('prefers-reduced-motion: reduce'),
  'app.css must respect OS reduced-motion settings (DESIGN.md §1 & §9)'
);
console.log('[OK] DESIGN.md §9: Focus ring (*:focus-visible, 2px solid --gtt-color-focus, 2px offset) and reduced-motion rules verified.');

// 2. Verify Landmarks, Skip Link & ARIA Labels in index.html (DESIGN.md §9)
assert(
  indexHtml.includes('href="#main-content" class="skip-link"'),
  'index.html must provide a skip-to-main link for keyboard navigation'
);
assert(
  indexHtml.includes('role="navigation" aria-label="Main Navigation"'),
  'Sidebar must have role="navigation" and aria-label'
);
assert(
  indexHtml.includes('role="banner" aria-label="Workspace Header"'),
  'Top bar must have role="banner" and aria-label'
);
assert(
  indexHtml.includes('<main id="main-content" class="sheet" aria-live="polite">'),
  'Main workspace must be a <main> landmark with aria-live="polite"'
);
assert(
  indexHtml.includes('id="toast-container" class="toast-container" aria-live="polite"'),
  'Toast container must have aria-live="polite" for screen-reader announcements'
);
console.log('[OK] DESIGN.md §9: HTML landmarks (navigation, banner, main, live regions) and skip link verified.');

// 3. Load app.js in VM Sandbox with Focus Tracking
const mainContentMock = { innerHTML: '' };
const viewTitleMock = { textContent: '' };
const topbarScopeMock = { textContent: '' };
const focusedElementLog = [];

function makeFocusableElement(id) {
  return {
    id,
    innerHTML: '',
    textContent: '',
    addEventListener: () => {},
    appendChild: () => {},
    classList: { add: () => {}, remove: () => {}, toggle: () => {} },
    focus: () => {
      focusedElementLog.push(id);
    },
  };
}

const elementsById = {
  'main-content': mainContentMock,
  'view-title': viewTitleMock,
  'topbar-scope': topbarScopeMock,
  'btn-cancel-stop': makeFocusableElement('btn-cancel-stop'),
  'btn-confirm-stop': makeFocusableElement('btn-confirm-stop'),
  'btn-cancel-managed-update': makeFocusableElement('btn-cancel-managed-update'),
  'btn-confirm-managed-update': makeFocusableElement('btn-confirm-managed-update'),
};

const windowMock = {
  addEventListener: () => {},
  document: {
    addEventListener: () => {},
    getElementById: (id) => elementsById[id] || makeFocusableElement(id),
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
  handleGlobalKeyDown,
  navigateTo,
  renderOnboardingView,
  renderChooseGameView,
  renderLocalConfigView,
  renderDashboardView,
  renderInvitationsView,
  renderTroubleshooterView,
  renderManagedServerView,
  renderSignInHandoffView,
  renderAccountView,
  state,
} = sandbox;

// 4. Verify Screen-Reader Semantics Across All Rendered Views (DESIGN.md §§6, 9)
const viewHtmlChecks = [
  ['onboarding', renderOnboardingView(), ['role="note"', 'aria-label="Privacy and encryption disclosure"']],
  ['signin-handoff', renderSignInHandoffView(), ['role="status"', 'aria-live="polite"']],
  ['choose-game', renderChooseGameView(), ['role="list"', 'role="listitem"', 'tabindex="0"']],
  ['local-config', renderLocalConfigView(), ['for="host-game"', 'for="port-game"', 'aria-invalid="false"', 'aria-describedby="err-port-game"']],
  ['dashboard', renderDashboardView(), ['id="indicator-tunnel"', 'id="indicator-local-server"', 'id="indicator-players-seen"', 'id="indicator-managed-server"', 'role="dialog"', 'aria-modal="true"']],
  ['invitations', renderInvitationsView(), ['for="inv-expiry-select"', 'for="inv-max-claims"', 'for="sim-claim-secret"']],
  ['troubleshooter', renderTroubleshooterView(), ['for="select-error-code"', 'aria-label="Reviewed error guidance"']],
  ['managed-server', renderManagedServerView(), ['for="checkbox-accept-eula"', 'role="progressbar"', 'aria-valuenow=']],
  ['account', renderAccountView(), ['role="progressbar"', 'aria-valuenow=']],
];

for (const [viewName, html, requiredSnippets] of viewHtmlChecks) {
  for (const snippet of requiredSnippets) {
    assert(
      html.includes(snippet),
      `View "${viewName}" must include accessible attribute/landmark: ${snippet}`
    );
  }
}
console.log(`[OK] Screen-reader labels, form label associations, and ARIA attributes verified across all ${viewHtmlChecks.length} views.`);

// 5. Complete End-to-End Keyboard-Only Journey: Onboarding -> Choose Game -> Configure -> Start -> Share -> Stop (UX-004, T-086 Done-when)
// Step 5a: Start at Welcome/Onboarding (S1) and activate Sign-In via Enter
navigateTo('onboarding');
state.isSignedIn = false;
state.tunnelState = 'Not running';
state.playersSeenCount = 0;

assert.strictEqual(state.currentView, 'onboarding');
assert.strictEqual(dispatchKeyAction('btn-start-signin', 'Enter'), true);
assert.strictEqual(state.currentView, 'signin-handoff', 'Enter on #btn-start-signin must navigate to S2 signin-handoff');

// Step 5b: Complete browser sign-in handoff (S2) via Enter -> arrives at Choose Game (S4)
assert.strictEqual(dispatchKeyAction('btn-simulate-auth-success', 'Enter'), true);
assert.strictEqual(state.isSignedIn, true, 'User must be signed in after S2 completion');
assert.strictEqual(state.currentView, 'choose-game', 'Completing S2 sign-in via keyboard must navigate to S4 choose-game');

// Step 5c: Select a game card (S4) via Space key -> arrives at Local Server Configuration (S5)
assert.strictEqual(dispatchKeyAction('profile-card:terraria', ' '), true);
assert.strictEqual(state.selectedProfile.id, 'terraria', 'Space on game card must select Terraria profile');
assert.strictEqual(state.currentView, 'local-config', 'Selecting a game card via keyboard must navigate to S5 local-config');

// Switch back to Minecraft: Java Edition via Enter on game card
navigateTo('choose-game');
assert.strictEqual(dispatchKeyAction('profile-card:minecraft-java', 'Enter'), true);
assert.strictEqual(state.selectedProfile.id, 'minecraft-java');
assert.strictEqual(state.currentView, 'local-config');

// Step 5d: Save local configuration (S5) via Enter -> arrives at Tunnel Dashboard (S7)
assert.strictEqual(dispatchKeyAction('btn-save-and-proceed', 'Enter'), true);
assert.strictEqual(state.currentView, 'dashboard', 'Enter on #btn-save-and-proceed must navigate to S7 dashboard');

// Step 5e: Start Tunnel (S7) via Enter
assert.strictEqual(dispatchKeyAction('btn-start-tunnel', 'Enter'), true);
assert.strictEqual(state.tunnelState, 'Connected', 'Enter on #btn-start-tunnel must start the tunnel');
assert(state.playersSeenCount > 0, 'Active session must have observed guest flows');

// Step 5f: Share / Create & Copy Invitation (S8) via Keyboard Only
assert.strictEqual(dispatchKeyAction('nav:invitations', 'Enter'), true);
assert.strictEqual(state.currentView, 'invitations', 'Keyboard nav must open S8 invitations screen');

assert.strictEqual(dispatchKeyAction('btn-create-invitation', 'Enter'), true);
assert(state.oneTimeInvitationBanner && state.oneTimeInvitationBanner.secret.startsWith('gtt_inv_'), 'Enter on #btn-create-invitation must create one-time secret');

assert.strictEqual(dispatchKeyAction('btn-copy-secret', 'Enter'), true);
assert.strictEqual(dispatchKeyAction('btn-dismiss-secret', 'Enter'), true);
assert.strictEqual(state.oneTimeInvitationBanner, null, 'Enter on #btn-dismiss-secret must dismiss the one-time banner');

// Step 5g: Return to Dashboard (S7) and Stop Tunnel via Keyboard Only (including Focus Trap & Escape Dismissal)
assert.strictEqual(dispatchKeyAction('nav:dashboard', 'Enter'), true);
assert.strictEqual(state.currentView, 'dashboard');

// Press Enter on #btn-stop-tunnel while guests are connected -> opens confirmation dialog with initial focus on safest action (#btn-cancel-stop)
assert.strictEqual(dispatchKeyAction('btn-stop-tunnel', 'Enter'), true);
assert.strictEqual(state.isStopModalOpen, true, 'Stop confirmation dialog must open when players are seen');
assert.strictEqual(
  state.activeFocusedElementId,
  'btn-cancel-stop',
  'Initial focus in stop confirmation dialog must land on the safest action (#btn-cancel-stop, DESIGN.md §7)'
);

// Press Tab inside modal -> focus traps between #btn-cancel-stop and #btn-confirm-stop
assert.strictEqual(dispatchKeyAction('', 'Tab'), true);
assert.strictEqual(state.activeFocusedElementId, 'btn-confirm-stop', 'Tab must move focus to second button inside modal');
assert.strictEqual(dispatchKeyAction('', 'Tab'), true);
assert.strictEqual(state.activeFocusedElementId, 'btn-cancel-stop', 'Tab must wrap focus back to first button inside modal (focus trap)');

// Press Escape -> closes confirmation dialog without stopping the tunnel (DESIGN.md §7 & §9)
assert.strictEqual(handleGlobalKeyDown({ key: 'Escape', preventDefault: () => {} }), true);
assert.strictEqual(state.isStopModalOpen, false, 'Escape must close the stop confirmation dialog');
assert.strictEqual(state.tunnelState, 'Connected', 'Tunnel must remain Connected when dialog is dismissed with Escape');

// Open stop dialog again via Enter and confirm stop via Enter on #btn-confirm-stop
assert.strictEqual(dispatchKeyAction('btn-stop-tunnel', 'Enter'), true);
assert.strictEqual(state.isStopModalOpen, true);
assert.strictEqual(dispatchKeyAction('btn-confirm-stop', 'Enter'), true);
assert.strictEqual(state.isStopModalOpen, false, 'Dialog must close after confirming stop');
assert.strictEqual(state.tunnelState, 'Not running', 'Tunnel must stop after keyboard confirmation');
assert.strictEqual(state.playersSeenCount, 0, 'Players seen count must reset to 0 after stop');

console.log('[OK] UX-004: Full host flow (onboarding -> sign-in -> choose game -> configure -> start -> share -> stop) completed by keyboard only.');

console.log('\n===================================================================');
console.log(' ALL ACCESSIBILITY & KEYBOARD-ONLY TESTS PASSED (EXIT 0)           ');
console.log('===================================================================');
