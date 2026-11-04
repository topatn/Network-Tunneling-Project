# GTT Desktop Application Shell (T-080 & T-081)

**Status:** IMPLEMENTED & LOCALLY VERIFIED.  
**Authoritative Requirements:** `UX-005` (Contrast & Color Tokens), `UX-007` (Light/Dark Theme Support), `UX-001` (First-Run Onboarding), `UX-002` (3-Part Independent Status Indicators), `UX-006` (Stop Confirmation with Active Guests), `FR-001` (Browser Sign-in), `PRIV-005` & `SEC-011` (Verbatim Encryption Disclosure).

---

## 1. How to Test on Your Desktop

You can test the GTT Desktop UI immediately on your Windows desktop using either method below:

### Method 1: Local HTTP Server (Recommended)
From the repository root, start the zero-dependency local preview server (defaults to port `5173`, automatically tries the next port if busy, or accepts any custom port):

```powershell
node apps/desktop/server.js
# Or specify any port:
# node apps/desktop/server.js 8085
```

Then open your browser to:
👉 **[http://127.0.0.1:5173](http://127.0.0.1:5173)** (or the port logged in your terminal)

### Method 2: Direct File Open
You can also open the file directly in Microsoft Edge, Google Chrome, or any browser:
👉 `file:///c:/Users/chris/Documents/Network Tunneling Project/apps/desktop/index.html`

---

## 2. Interactive Features to Test

1. **Light & Dark Theme Toggle (`UX-007`, `UX-005`):**
   - In the sidebar footer, switch between **Follow System**, **Light Theme**, and **Dark Theme**.
   - Contrast is verified across all 22 defined token pairs (`check_contrast.py`).
2. **First-Run Onboarding (S1, `FR-001`, `UX-001`):**
   - Notice the prominent plain-language explanation and verbatim encryption disclosure from `docs/privacy-notice.md`.
   - Click **"Sign In with Browser"** to test the system browser handoff screen (S2).
3. **Three-Layer Independent Status Machine (S7, `UX-002`, `FR-010`):**
   - Click **"Tunnel Dashboard"** in the sidebar.
   - Observe the three distinct status indicators (Tunnel Connection, Local Game Server, Guest Traffic Observed).
   - In accordance with `DESIGN.md` §1 and `user-flows.md` §0, these three indicators are never combined into a single status.
4. **Game Profile Selection (S4, `FR-002`, `GAME-009`):**
   - Click between Minecraft Java, Minecraft Bedrock, Terraria, and Left 4 Dead 2 to observe dynamic port and protocol switching.
5. **Tunnel Lifecycle & Active Player Protection (`UX-006`):**
   - Click **"Start Tunnel"**: status changes from `Not running` -> `Connecting` (spinner) -> `Connected` (green check).
   - Click **"Copy Address"**: copies public address with toast notification.
   - Click **"Stop Tunnel"**: because active guest traffic is observed (3 players seen), the app requires explicit confirmation: *"Disconnect Active Players? Stopping now will immediately disconnect them from the game."*
