# ProtonDrive-Linux

An unofficial native Linux client for [Proton Drive](https://proton.me/drive),
written in Rust with a Go CGO bridge to the official Proton API library.

Your Proton Drive appears as a **FUSE virtual filesystem at `/mnt/ProtonDrive`**
(or `~/ProtonDrive`) so it shows up in your file manager like the official
Windows and macOS clients. Files are **fetched on demand** — only the directory
tree is kept locally; file content is downloaded the moment you open it.

> ⚠️ **Unofficial third-party app.** Not affiliated with or endorsed by Proton AG.

---

## Features

- **On-demand FUSE VFS** — directory tree cached locally; file bodies downloaded
  only when accessed.
- **Streaming BFS scan** — initial sync walks the entire Drive tree incrementally
  via the Drive Events API (no recursive full-walk, respects Proton's ToS).
- **Batch SQLite state** — mapping table written in single transactions with
  `WAL + synchronous=NORMAL`; handles 100 k+ file drives without stalling.
- **GTK 4 + libadwaita** setup wizard.
- **System tray** icon (StatusNotifierItem via `ksni`) — works on **GNOME,
  KDE Plasma, Budgie, and Cinnamon**.
- **Automatic CAPTCHA solving** — if Proton presents a Human Verification
  challenge (error 9001) the app solves the drag-puzzle automatically using
  an embedded WebKit view.
- Credentials and TOTP secret stored in your **system keyring** (libsecret /
  GNOME Keyring / KWallet). The raw password is **never** stored on disk.

---

## Installation

Pre-built packages are attached to every [GitHub Release](https://github.com/0xnullsect0r/ProtonDrive-Linux/releases).
Pick the one for your distro.

> The snippets below use bash syntax (`VER=X.Y.Z` + `${VER}`). On **fish**,
> use `set VER X.Y.Z` and `{$VER}` instead — or just paste the URL with the
> version inlined.

### Debian / Ubuntu / Mint / Pop!\_OS / Elementary / Zorin

```bash
# Replace X.Y.Z with the latest release version (e.g. 0.1.22)
VER=X.Y.Z
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/protondrive-linux_${VER}_amd64.deb
sudo apt install ./protondrive-linux_${VER}_amd64.deb
```

### Fedora / RHEL / Rocky / AlmaLinux / openSUSE Tumbleweed / openSUSE Leap

```bash
VER=X.Y.Z
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/protondrive-linux-${VER}-1.fc41.x86_64.rpm

# Fedora / RHEL family
sudo dnf install ./protondrive-linux-${VER}-1.fc41.x86_64.rpm

# openSUSE
sudo zypper install --allow-unsigned-rpm ./protondrive-linux-${VER}-1.fc41.x86_64.rpm
```

### Arch / CachyOS / Manjaro / EndeavourOS

The release ships a ready-to-use `PKGBUILD`; build it locally with `makepkg`:

```bash
VER=X.Y.Z
mkdir protondrive-linux && cd protondrive-linux
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/PKGBUILD
makepkg -si
```

Or install directly from the AUR:

```bash
yay -S protondrive-linux
# or: paru -S protondrive-linux
```

### Flatpak (any distro with Flathub-style runtimes)

```bash
VER=X.Y.Z
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/protondrive-linux-${VER}.flatpak
flatpak install --user ./protondrive-linux-${VER}.flatpak
flatpak run me.proton.drive.Linux
```

(You may need `flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo` first to pull the GNOME runtime.)

### AppImage (any glibc distro)

```bash
VER=X.Y.Z
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/protondrive-linux-${VER}-x86_64.AppImage
chmod +x protondrive-linux-${VER}-x86_64.AppImage
./protondrive-linux-${VER}-x86_64.AppImage
```

---

## First run

1. Launch **Proton Drive Linux** from your application menu (or run
   `protondrive` from a terminal).
2. The setup wizard asks for:
   - Proton **email** and **password**
   - **TOTP secret** — the Base32 *key* used to generate codes,
     not a 6-digit code (export it from your authenticator app)
   - **Sync folder** (default `~/ProtonDrive`)
3. After saving, the tray icon appears and your Drive tree is populated.
   Open your file manager and look for **ProtonDrive** in the sidebar.

Credentials are stored in your **system keyring** (GNOME Keyring / KWallet),
not in plain text on disk.

---

## Building from source

System dependencies (Arch names; use the equivalents on your distro):

```
rust >= 1.88   gtk4   libadwaita   fuse3   libsecret   openssl   pkgconf   go >= 1.22
```

```bash
git clone https://github.com/0xnullsect0r/ProtonDrive-Linux.git
cd ProtonDrive-Linux
cargo build --release
./target/release/protondrive
```

---

## Workspace layout

```
crates/
  protondrive-core     library: auth, API client, crypto, cache, sync, types
  protondrive-bridge   CGO bridge to henrybear327/Proton-API-Bridge (Go)
    go/                Go source: bridge.go + proton-api-bridge-fork/
    go/integration/    Live integration test (separate Go module)
  protondrive-ui       GTK4 + libadwaita setup/settings UI + tray
                       (binary `protondrive` — the user-facing app)
  protondrive-sync     4-stage sync engine (reconcile → conflict → propagate → consolidate)
  protondrive-cli      headless CLI for scripting / debugging
                       (binary `protondrive-cli`)
packaging/
  deb/  rpm/  arch/  flatpak/  appimage/   per-format build assets
```

---

## CI Integration Testing

The repository includes a live integration test at
`crates/protondrive-bridge/go/integration/`. It logs into the real Proton
Drive API, lists the root folder, and asserts at least one item is returned.

To enable it, add three secrets to your fork/repository in
**Settings → Secrets and variables → Actions**:

| Secret name       | Value                                                              |
|-------------------|--------------------------------------------------------------------|
| `PROTON_USER`     | Your Proton account email                                          |
| `PROTON_PASSWORD` | Your Proton account password                                       |
| `PROTON_KEY`      | Your TOTP secret key (Base32, e.g. `JBSWY3DPEHPK3PXP…`) — **not** a 6-digit code |

The integration workflow (`.github/workflows/integration.yml`) runs on every
push to `main` and on manual dispatch. It is **silently skipped** when the
secrets are absent (e.g. on forks or PRs from external contributors).

If Proton presents a Human Verification (CAPTCHA) challenge during the test,
the solver uses headless Chromium to drag the puzzle piece to the correct slot
using normalised cross-correlation template matching and a realistic Bézier
mouse path with random jitter.

---

## Releases (for maintainers)

Push a tag of the form **`vX.Y.Z`** (strict semver, three numeric components,
e.g. `v0.1.0`, `v1.2.3`) to trigger the release workflow. CI rewrites the
workspace version automatically — you do **not** need to bump `Cargo.toml`
yourself. Any tag not matching `vX.Y.Z` is ignored.

```bash
git tag v0.1.23
git push origin v0.1.23
```

---

## License

GPL-3.0-or-later.

---

## Troubleshooting

### "For security reasons, please complete the CAPTCHA" / Code 9001

Since v0.1.10+, the app handles this automatically using an embedded headless
browser. If the auto-solver fails (e.g. Proton changes the captcha layout),
you can work around it manually:

1. Open <https://account.proton.me> in your browser **on this same machine**
   and sign in there once, solving any CAPTCHA Proton presents.
2. Immediately come back to ProtonDrive-Linux and click **Sign in** again.

Your IP will be trusted for roughly 24 hours afterwards. If you switch
networks (e.g. VPN, mobile hotspot), repeat the browser sign-in.

### Code 2064 / "Invalid Section Name"

Proton rejected the request as malformed because the `x-pm-appversion`
header is unrecognised. Update to the latest ProtonDrive-Linux release —
this is fixed in v0.1.8+.

### TOTP / 2FA code rejected

Make sure you pasted the **secret key** (Base32, e.g. `JBSWY3DPEHPK3PXP…`),
not a 6-digit code. Strip spaces. The current generated code is shown live
on the Two-Factor tab — verify it matches your authenticator app before
signing in.

### Files not appearing in the folder

ProtonDrive-Linux uses on-demand FUSE — the directory tree populates
immediately, but file content is fetched only when you open the file.
If the folder appears empty, check:

1. The tray icon is not showing an error (red ×).
2. Run `journalctl --user -u protondrive -f` to see live log output.
3. Ensure the FUSE mount is active: `mount | grep ProtonDrive`.

