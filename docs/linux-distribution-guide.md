# Linux Distribution & Packaging Guide for Game Tunnel Tooling (GTT)

**Status:** Active  
**Deliverables:** Standalone Binary Tarball (`.tar.gz`), Debian Package (`.deb`), AppImage Bundle (`.AppDir`), and systemd unit (`gtt.service`)  
**Packaging Automation:** [`packaging/linux/build_linux_dist.py`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/packaging/linux/build_linux_dist.py)  
**Verification Suite:** [`scripts/verify_linux_packaging.py`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/scripts/verify_linux_packaging.py)  

---

## 1. Overview

While GTT provides a native WebView2 desktop shell for Windows, the underlying core engine (`crates/engine`), game server manager (`crates/server-manager`), and embedded web UI runtime (`apps/desktop`) are fully cross-platform and run natively on Linux desktops and headless servers.

### Supported Linux Deployment Modes

1. **Linux Desktop (GUI Mode):** Launches the GTT web dashboard in an isolated app-mode browser window using Chromium, Google Chrome, Brave, or Microsoft Edge for Linux, with fallback to `xdg-open`.
2. **Headless Linux Server / VPS (Daemon Mode):** Runs purely as a background daemon (`--headless` or `--no-open`) controlled via REST API and web browser, managed by systemd (`gtt.service`).
3. **Container / VPS:** Binds to custom network interfaces (`--host <ip>` and `--port <port>`) with zero GUI requirements.

---

## 2. Package Formats & Distribution Artifacts

The packaging tool ([`packaging/linux/build_linux_dist.py`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/packaging/linux/build_linux_dist.py)) outputs all artifacts into `dist/linux/`:

| Artifact | Format | Target Environments | Description |
|---|---|---|---|
| `gtt-v0.1.0-linux-x86_64.tar.gz` | Portable Binary Tarball | Any Linux distribution (Arch, Fedora, Alpine, Debian) | Self-contained archive with `bin/gtt`, desktop entry, scalable icon, systemd service, and installation script |
| `gtt_0.1.0_amd64.deb` | Debian Binary Package (`ar`) | Debian, Ubuntu, Linux Mint, Pop!_OS | Native package installing to `/usr/bin/gtt`, `/usr/share/applications`, and `/lib/systemd/system` |
| `gtt.AppDir/` | AppImage Bundle Directory | Any Linux desktop | Prepared `AppDir` containing `AppRun`, desktop metadata, and binaries ready for `appimagetool` |
| `SHA256SUMS` | Plain text | All platforms | Cryptographic SHA-256 integrity checksums for all release artifacts |

---

## 3. Installation Instructions

### 3.1 Method A: Debian / Ubuntu Package (`.deb`)

```bash
# 1. Install the Debian package
sudo apt install ./gtt_0.1.0_amd64.deb
# or
sudo dpkg -i gtt_0.1.0_amd64.deb

# 2. Verify installation
gtt --self-check
```

### 3.2 Method B: Standalone Tarball (`.tar.gz`)

The standalone tarball can be installed system-wide (as root) or per-user (without root privileges):

```bash
# Extract the tarball
tar -xzf gtt-v0.1.0-linux-x86_64.tar.gz
cd gtt-v0.1.0/

# System-wide installation (installs to /usr/local/bin)
sudo ./install.sh

# Or Per-User installation (installs to ~/.local/bin and ~/.local/share/applications)
./install.sh
```

### 3.3 Method C: AppImage (Single Executable)

```bash
# If using appimagetool to generate a single-file AppImage:
appimagetool dist/linux/gtt.AppDir gtt-x86_64.AppImage

# Make executable and run
chmod +x gtt-x86_64.AppImage
./gtt-x86_64.AppImage
```

---

## 4. Running GTT on Linux

### 4.1 Running with Desktop GUI

From your application launcher, search for **Game Tunnel Tooling**, or run:
```bash
gtt
```
GTT will start the embedded loopback engine bridge on `http://127.0.0.1:5173` and launch a clean standalone application window.

### 4.2 Running on Headless Linux Servers / VPS

To run GTT on a dedicated Linux server without any display server (X11/Wayland):

```bash
# Run directly in foreground
gtt --headless --port 5173 --host 0.0.0.0
```

### 4.3 Running as a Systemd Service

GTT includes a pre-configured systemd unit ([`packaging/linux/gtt.service`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/packaging/linux/gtt.service)):

```bash
# Enable and start system-wide service
sudo systemctl daemon-reload
sudo systemctl enable --now gtt.service

# Check service status
sudo systemctl status gtt.service

# View live application logs
journalctl -u gtt.service -f
```

For unprivileged user services:
```bash
systemctl --user enable --now gtt.service
journalctl --user -u gtt.service -f
```

---

## 5. Uninstallation

```bash
# If installed via .deb:
sudo apt remove gtt
# or
sudo dpkg -r gtt

# If installed via tarball install.sh:
./uninstall.sh

# To completely purge server worlds, backups, and user data:
./uninstall.sh --remove-user-data
```

---

## 6. Building Distribution Packages from Source

To build Linux distribution packages from the repository:

```powershell
# 1. Compile the desktop/engine binary
cargo build --release -p gtt-desktop

# 2. Run packaging automation
python packaging/linux/build_linux_dist.py --binary target/release/gtt-desktop.exe --out-dir dist/linux

# 3. Verify packages and metadata
python scripts/verify_linux_packaging.py
```
