# DESIGN.md — Desktop Design System

**Status:** DRAFT v0.1 — **provisional choices**. Authoritative for the visual design system. Flows: `user-flows.md`. Principles and navigation: `ux.md`.
**Last updated:** 2026-10-16

**Provisional means:** the product owner has not chosen brand colors, themes, or window behavior, so neutral defaults were picked. Changing a token value is cheap as long as the contrast check below is re-run (T-080).

**Open decision:** the front-end framework for the Tauri UI is not chosen (it follows the desktop spike, T-018). Tokens are therefore defined as CSS custom properties, which work with any framework. A Tailwind mapping is shown as an *example* only; Tailwind is not selected.

## 1. Philosophy and hierarchy

1. **Truthful status first.** The most prominent element on the dashboard is the three-part status (tunnel, local server, players seen). Nothing decorative competes with it.
2. **Plain language.** No networking jargon in primary UI (NAT, relay, QUIC). Technical detail lives behind a "Details" disclosure.
3. **One primary action per screen.** Start, Share, or Stop; destructive actions are visually secondary until confirmed.
4. **Calm by default.** Motion only to show state changes; respects reduced-motion settings.
5. **Never color alone.** Every status has an icon and text.

## 2. Color tokens

Values are provisional. Both themes are defined; the app follows the operating-system theme by default (UX-007, P2). Contrast was **computed**, see §3.

| Token | Light | Dark | Purpose |
|---|---|---|---|
| `--gtt-color-bg` | `#FFFFFF` | `#12151B` | App background |
| `--gtt-color-surface` | `#F4F6F9` | `#1A1E26` | Panels, sidebar |
| `--gtt-color-raised` | `#FFFFFF` | `#222732` | Cards, dialogs, menus |
| `--gtt-color-border` | `#D5DAE1` | `#343B49` | Decorative dividers only (not used to identify controls) |
| `--gtt-color-control-border` | `#6B7482` | `#8A94A6` | Borders of inputs, toggles, buttons that need to be identified |
| `--gtt-color-text` | `#1B1F27` | `#E8EBF0` | Primary text |
| `--gtt-color-text-muted` | `#455060` | `#B4BCC9` | Secondary text, hints |
| `--gtt-color-accent` | `#2358D4` | `#8AB0FF` | Primary actions, links, selected state |
| `--gtt-color-on-accent` | `#FFFFFF` | `#0B1220` | Text/icons on accent fill |
| `--gtt-color-focus` | `#1D4ED8` | `#9DBBFF` | Focus ring |
| `--gtt-color-ok` | `#146C36` | `#66D18C` | Success / connected / reachable |
| `--gtt-color-warn` | `#7A4F00` | `#F2BE55` | Degraded / attention |
| `--gtt-color-error` | `#B3261E` | `#FF9088` | Failure / destructive |
| `--gtt-color-info` | `#1A56A0` | `#86B9FF` | Neutral information / in progress |
| `--gtt-color-neutral` | `#4F5A6A` | `#A5AEBD` | Idle / disconnected / unknown |

## 3. Contrast verification (computed)

Method: WCAG 2.x relative-luminance contrast formula. Thresholds: 4.5:1 for normal text, 3:1 for non-text UI boundaries and focus indicators.

**Verified 2026-10-17** by `scripts/check_contrast.py`, which recomputes every ratio below from the token table in §2 rather than trusting the numbers here. That script first validates its own implementation against the reference pairs this section previously cited (black/white = 21.00; #767676 on white = 4.54; #777777 on white = 4.48), then recomputes all 22 pairs. **All 22 matched.** It also rejects the table if a ratio is overstated or a threshold is wrong — confirmed by planting both defects and watching each one fail. It runs in CI and covers every pair in the table below, so a pair added here without being checked is an error rather than a silent gap.

**Not verified, and unchanged:** hover, disabled, and high-contrast-mode states are not in the table and are not checked; the WCAG version to cite in release documents should be confirmed when T-080 runs.

| Theme | Pair | Ratio | Needed | Result |
|---|---|---|---|---|
| light | text on bg | 16.51 | 4.5 | pass |
| light | muted text on surface | 7.55 | 4.5 | pass |
| light | accent text on bg | 6.14 | 4.5 | pass |
| light | on-accent on accent | 6.14 | 4.5 | pass |
| light | ok on raised | 6.51 | 4.5 | pass |
| light | warn on raised | 7.13 | 4.5 | pass |
| light | error on raised | 6.54 | 4.5 | pass |
| light | info on raised | 7.28 | 4.5 | pass |
| light | neutral on raised | 6.99 | 4.5 | pass |
| light | control border on bg | 4.72 | 3.0 | pass |
| light | focus ring on surface | 6.19 | 3.0 | pass |
| dark | text on bg | 15.30 | 4.5 | pass |
| dark | muted text on surface | 8.73 | 4.5 | pass |
| dark | accent text on bg | 8.47 | 4.5 | pass |
| dark | on-accent on accent | 8.68 | 4.5 | pass |
| dark | ok on raised | 7.88 | 4.5 | pass |
| dark | warn on raised | 8.74 | 4.5 | pass |
| dark | error on raised | 6.83 | 4.5 | pass |
| dark | info on raised | 7.40 | 4.5 | pass |
| dark | neutral on raised | 6.69 | 4.5 | pass |
| dark | control border on bg | 5.98 | 3.0 | pass |
| dark | focus ring on surface | 8.74 | 3.0 | pass |

Not verified: pairs not listed (for example text on hover or disabled states), and contrast of any imagery or third-party icons.

## 4. Typography

- **Font:** the system UI font stack (`"Segoe UI Variable", "Segoe UI", system-ui, sans-serif`). No font files are bundled, so there is no licensing or update burden. Monospace for addresses and ports: `ui-monospace, "Cascadia Mono", Consolas, monospace`.
- **Scale (px, provisional):** 12 caption, 14 body, 16 emphasis body, 20 section title, 24 screen title, 32 display (onboarding only). Line height 1.4 for body, 1.25 for titles.
- **Weights:** 400 body, 600 titles and buttons.
- Text must remain usable when the OS text scaling is increased; layouts use relative units.

## 5. Spacing, layout, shape

- **Spacing scale (px):** 4, 8, 12, 16, 24, 32, 48 as `--gtt-space-1` to `--gtt-space-7`.
- **Radii:** 4 (controls), 8 (cards), 12 (dialogs). **Borders:** 1 px decorative; controls use `--gtt-color-control-border`.
- **Elevation:** three levels. Level 0 flat; level 1 cards (light: subtle shadow, dark: slightly lighter surface); level 2 menus and dialogs (shadow plus border). Dark theme relies on surface lightness, not shadows.
- **Icons:** one outline icon family, 16 and 20 px; the specific library is a dependency decision recorded with its purpose and maintenance notes (T-030). Icons that carry meaning have text labels or accessible names.
- **Window:** resizable; minimum size 720×520 px *(provisional)*. Below 900 px width the sidebar collapses to icons; the status panel stays at the top at every width.

## 6. Status indicators

Status is shown with an icon, a text label, and a color token. Mapping from the engine state machine (`contracts.md` §12):

| Shown label | Engine state(s) | Token | Icon (shape) | Live-region politeness |
|---|---|---|---|---|
| Not running | Idle, Stopped | neutral | hollow circle | none |
| Connecting | Connecting, Authenticating | info | spinner | polite |
| Connected | Active | ok | check circle | polite |
| Degraded | Degraded | warn | triangle | polite |
| Reconnecting | Reconnecting | warn | circular arrows | polite |
| Failed | Failed | error | octagon / cross | assertive |

Local server indicator: **Reachable** (ok, check), **Not reachable** (error, cross), **Checking** (info, spinner), **Unknown** (neutral, question mark; used when no probe exists, never shown as reachable).
Players indicator: **No traffic seen** (neutral), **N guests seen** (ok, labeled "observed traffic"), **Not available** (neutral).

Managed server indicator (added 2026-10-17): **Not set up** (neutral), **Downloading** / **Verifying** / **Configuring** / **Starting** (info, spinner, with a progress value), **Running** (ok, check), **Crashed** (error, cross, with the log excerpt behind "Details"), **Updating** (info, spinner), **Failed** (error, octagon, with cause and action). This is a **fourth, separate** component. It never shares an icon or color with the three above, and "Running" must not make the *Local server* indicator read as anything other than what the probe found (`SRV-006`, FR-010, UX-002).

The three indicators are separate components and never share one icon or color (FR-010, UX-002). "Connected" must not imply that the game server works or that anyone has joined.

## 7. Components and patterns

| Component | Rules |
|---|---|
| Button | Primary (accent fill), secondary (control border), destructive (error text, confirm required), ghost. Minimum target 32×32 px. Disabled state keeps contrast for its label ≥ 3:1 and exposes a reason via accessible description |
| Text field | Visible label above; hint below; control border token; error state adds icon and message, not color alone |
| Select / Toggle | Native semantics (listbox/switch); toggle label states the effect ("Expose query port") |
| Status badge | Icon + label per §6 |
| Copy field | Monospace value, "Copy" button, confirmation announced to screen readers |
| Card | Raised surface, level 1 |
| Dialog | Level 2, focus trapped, Escape closes unless destructive-in-progress, initial focus on the safest action |
| Banner | Persistent, for failures needing attention (relay unavailable); includes the next action |
| Toast | Transient, for confirmations only (copied, revoked); never the only place an error appears |
| Empty state | Icon, one sentence, one primary action |
| Loading | Spinner with text for operations under a few seconds; skeleton for lists; long operations show what is happening and a cancel option |
| List / table | Row actions reachable by keyboard; destructive row actions require confirmation |

### Form validation
Validate on blur and on submit. Messages say what is wrong and how to fix it, e.g. "Enter a port between 1 and 65535" or "This address is not allowed. Use 127.0.0.1 or confirm a LAN address." (policy: `contracts.md` §13). Never clear the user's input on error. The first invalid field receives focus on submit.

### Confirmations and notifications
Confirm: stopping a tunnel while guests are seen (UX-006), revoking an invitation, deleting the account, enabling a non-default exposure (query/RCON) with a one-line explanation of the risk, accepting a game's terms before provisioning, and applying a game server update while a tunnel is active (which disconnects guests). Do not confirm: copy, refresh, navigation.

## 8. Standard states

| State | Pattern |
|---|---|
| Empty | Empty-state component with a primary action ("Create your first tunnel") |
| Loading | Spinner/skeleton; the screen remains usable where possible |
| Error | Banner or inline message with cause and action (`user-flows.md`), plus "Details" |
| Retry | Explicit Retry button; automatic retries show a countdown or "Reconnecting" |
| Success | Status change plus a toast only for actions that otherwise show no visible result |
| Multi-step task (added 2026-10-17) | Named steps with per-step state (download, verify, configure, start), progress value, and a cancel option that states what already happened |

## 9. Keyboard and accessibility requirements

- Every control is reachable and operable by keyboard in a logical order; no keyboard traps; Escape closes dialogs and menus; Enter activates the focused primary action.
- A visible focus ring (`--gtt-color-focus`, 2 px with 2 px offset) is shown on every interactive element.
- Status changes are exposed to assistive technology through live regions per §6; the dashboard has a heading structure and landmarks.
- Respect OS reduced-motion, high-contrast, and text-size settings. High-contrast behavior is **not yet verified** and needs testing in T-086.
- Targets at least 24×24 px with adequate spacing; 32×32 recommended.
- Accessibility tests: keyboard-only run through onboarding, start, share, stop (UX-004); automated contrast check (UX-005); screen-reader spot check (T-086).

## 10. Naming and mapping to the framework

- Tokens: `--gtt-<category>-<name>` CSS variables on `:root`, with a dark override under the OS color-scheme query and a manual theme attribute.
- Components: PascalCase names (`StatusPanel`, `CopyField`); CSS classes kebab-case with a component prefix (`status-panel__title`).
- Token file is the single source: `apps/desktop/src/tokens.css` (does not exist yet). Any framework mapping reads these variables.
- Example mapping if Tailwind were chosen (illustrative, **not decided**): theme colors reference the variables, e.g. `colors: { surface: "var(--gtt-color-surface)", accent: "var(--gtt-color-accent)" }`.

## 11. Related documents

`ux.md`, `user-flows.md`, `requirements.md` (UX-001 to UX-007), `tasks.md` (T-080 to T-087).
