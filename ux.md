# ux.md — UX Principles and Navigation

**Status:** DRAFT. Companion file. Canonical journeys are in `user-flows.md`; visual rules in `DESIGN.md`. This file holds only principles and the navigation model.

## Principles

1. **Say what is actually known.** Keep three facts separate: tunnel up, local game server reachable, guest traffic observed. None implies the others (`user-flows.md` §0).
2. **Plain language first.** Technical detail sits behind "Details".
3. **Show exposure before it happens.** Before a tunnel starts, list exactly what will be reachable and by whom.
4. **Safe defaults.** Optional ports are off; invitations expire; revocation is one click.
5. **Every error has a cause and a next step** (UX-003).
6. **Accessible by default**: keyboard operation, visible focus, no color-only meaning (UX-004, UX-005).
7. **No surprises about data.** The app states what the relay and operator can see (SEC-011, PRIV-005).
8. **Don't make the host be a server administrator** *(added 2026-10-17)*. Setting up a game server should be one offered, clearly labelled step, not a project. Manual configuration stays available and equally supported, because a host's own setup is legitimate and sometimes deliberate. The app never accepts a game's terms on the host's behalf and never asks for administrator rights.

## Navigation model

| Area | Contents | Reached from |
|---|---|---|
| Home | Tunnel list, "New tunnel" | App start (signed in) |
| New tunnel flow | Game → Local server → Review and start | Home |
| Managed server setup | Offered during Local server for provisionable games only: download, verify, configure, start; then world, backup, update | Local server screen |
| Tunnel dashboard | Status, address, invitations, details | Home row, end of new-tunnel flow |
| Troubleshooter | Guided checks | Dashboard banner, Help |
| Settings | Account, diagnostics, updates, theme, privacy notice | Sidebar (collapses to icons on narrow windows) |

Primary navigation is a left sidebar with Home and Settings. The new-tunnel flow is a linear sequence with Back at every step and no data lost on Back. Dialogs are used only for confirmations, sharing, and updates.

## Links

`user-flows.md` (canonical journeys), `DESIGN.md`, `requirements.md` (UX-001 to UX-007).
