/**
 * Automated UI Shell & Token Consistency Test (T-080, T-081, UX-005, UX-007, FR-001)
 */

const fs = require('fs');
const path = require('path');
const assert = require('assert');

const DESKTOP_DIR = path.resolve(__dirname, '..');
const TOKENS_PATH = path.join(DESKTOP_DIR, 'src', 'tokens.css');
const APP_JS_PATH = path.join(DESKTOP_DIR, 'src', 'app.js');
const INDEX_HTML_PATH = path.join(DESKTOP_DIR, 'index.html');

console.log('Testing GTT Desktop UI Shell & Tokens...');

// 1. Verify tokens.css
assert(fs.existsSync(TOKENS_PATH), 'tokens.css must exist');
const tokensCss = fs.readFileSync(TOKENS_PATH, 'utf8');

const requiredTokens = [
  '--gtt-color-bg',
  '--gtt-color-surface',
  '--gtt-color-raised',
  '--gtt-color-border',
  '--gtt-color-control-border',
  '--gtt-color-text',
  '--gtt-color-text-muted',
  '--gtt-color-accent',
  '--gtt-color-on-accent',
  '--gtt-color-focus',
  '--gtt-color-ok',
  '--gtt-color-warn',
  '--gtt-color-error',
  '--gtt-color-info',
  '--gtt-color-neutral',
];

for (const tok of requiredTokens) {
  assert(tokensCss.includes(tok), `tokens.css missing token: ${tok}`);
}
console.log(`[OK] All ${requiredTokens.length} color tokens defined in tokens.css.`);

// Verify light and dark mode rules
assert(tokensCss.includes('prefers-color-scheme: dark'), 'Dark mode media query must be defined');
assert(tokensCss.includes('[data-theme="dark"]'), 'Manual dark theme selector must be defined');
assert(tokensCss.includes('[data-theme="light"]'), 'Manual light theme selector must be defined');
console.log('[OK] Light and Dark theme selectors verified (UX-007).');

// 2. Verify index.html
assert(fs.existsSync(INDEX_HTML_PATH), 'index.html must exist');
const indexHtml = fs.readFileSync(INDEX_HTML_PATH, 'utf8');
assert(indexHtml.includes('src/tokens.css'), 'index.html must load tokens.css');
assert(indexHtml.includes('src/app.css'), 'index.html must load app.css');
assert(indexHtml.includes('src/app.js'), 'index.html must load app.js');
assert(indexHtml.includes('theme-select'), 'index.html must include theme selector');
console.log('[OK] index.html structure and asset loading verified.');

// 3. Verify app.js disclosures and 3-part status
assert(fs.existsSync(APP_JS_PATH), 'app.js must exist');
const appJs = fs.readFileSync(APP_JS_PATH, 'utf8');

// Privacy & encryption verbatim string check (PRIV-005, SEC-011)
assert(appJs.includes('Your game traffic is encrypted between your PC and our relay servers.'), 'Verbatim encryption disclosure required in onboarding');
assert(appJs.includes('game payloads are never inspected, modified, or written to disk.'), 'Verbatim payload non-inspection disclosure required');
console.log('[OK] Verbatim encryption disclosure verified in onboarding (PRIV-005, SEC-011).');

// Three independent status indicators check (UX-002, FR-010)
assert(appJs.includes('Tunnel Connection'), 'Tunnel status indicator must exist');
assert(appJs.includes('Local Server'), 'Local server status indicator must exist');
assert(appJs.includes('Guest Traffic Observed'), 'Guest traffic observed status indicator must exist');
console.log('[OK] Three independent status layers verified (UX-002, FR-010).');

// Active players disconnect confirmation check (UX-006)
assert(appJs.includes('Disconnect Active Players?'), 'Stop confirmation required when guests active');
console.log('[OK] Stop tunnel confirmation modal verified (UX-006).');

console.log('\nALL DESKTOP UI SHELL & TOKEN TESTS PASSED (EXIT 0).');
