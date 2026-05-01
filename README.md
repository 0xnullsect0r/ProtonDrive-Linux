# ProtonDrive-Linux

An unofficial native Linux client for [Proton Drive](https://proton.me/drive),
written in Rust.

It mounts your Proton Drive as a **FUSE filesystem at `/mnt/ProtonDrive`** so
it appears as a device in your file manager — much like the official Windows
and macOS clients. Aggressive local cache, system-tray icon, and a
GTK 4 / libadwaita setup wizard.

> ⚠️ **Status:** early scaffolding. The architecture, FUSE skeleton, cache,
> setup UI, system tray, keyring, config, and CI/release pipeline are in
> place. The Proton-specific protocol pieces (SRP authentication, Drive REST
> endpoints, PGP key/block decryption) are stubbed and need to be filled in
> by porting from the official open-source clients at
> <https://github.com/ProtonDriveApps>.

---

## Features

- Mounts your Proton Drive as a **FUSE filesystem at `/mnt/ProtonDrive`**.
- Aggressive **local cache** (content-addressed blob store + SQLite metadata).
- **"Always available offline"** pinning per file or folder.
- Polls Proton for changes every **20 seconds**, plus *Refresh now* in the tray.
- **GTK 4 + libadwaita** setup window.
- **System tray** icon (StatusNotifierItem via `ksni`) — works on **GNOME,
  KDE Plasma, Budgie, and Cinnamon**.
- Credentials and TOTP secret stored in your **system keyring** (libsecret).

---

## Installation

Pre-built packages are attached to every [GitHub Release](https://github.com/0xnullsect0r/ProtonDrive-Linux/releases).
Pick the one for your distro.

### Debian / Ubuntu / Mint / Pop!\_OS / Elementary / Zorin

```bash
# Replace X.Y.Z with the latest release version (e.g. 0.1.5)
VER=X.Y.Z
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/protondrive-linux_${VER}_amd64.deb
sudo apt install ./protondrive-linux_${VER}_amd64.deb
```

### Fedora / RHEL / Rocky / AlmaLinux / openSUSE Tumbleweed / openSUSE Leap

```bash
VER=X.Y.Z
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/protondrive-linux-${VER}-1.x86_64.rpm

# Fedora / RHEL family
sudo dnf install ./protondrive-linux-${VER}-1.x86_64.rpm

# openSUSE
sudo zypper install ./protondrive-linux-${VER}-1.x86_64.rpm
```

### Arch / CachyOS / Manjaro / EndeavourOS

```bash
VER=X.Y.Z
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/protondrive-linux-${VER}-1-x86_64.pkg.tar.zst
sudo pacman -U protondrive-linux-${VER}-1-x86_64.pkg.tar.zst
```

Or build from the included `PKGBUILD`:

```bash
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/PKGBUILD
makepkg -si
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
curl -LO https://github.com/0xnullsect0r/ProtonDrive-Linux/releases/download/v${VER}/ProtonDrive-Linux-${VER}-x86_64.AppImage
chmod +x ProtonDrive-Linux-${VER}-x86_64.AppImage
./ProtonDrive-Linux-${VER}-x86_64.AppImage
```

The AppImage requires `fuse2` (or libfuse2t64 on Ubuntu 24.04+) at runtime:

```bash
# Debian/Ubuntu
sudo apt install libfuse2t64    # 24.04+ uses libfuse2t64; older uses libfuse2

# Fedora
sudo dnf install fuse-libs

# Arch
sudo pacman -S fuse2
```

---

## First run

1. Launch **Proton Drive Linux** from your application menu (or run
   `protondrive` from a terminal).
2. The setup wizard asks for:
   - Proton **email** and **password**
   - **TOTP secret** — the Base32 *key* used to generate codes,
     not a 6-digit code (export it from your authenticator app)
   - **Sync folder** (default `/mnt/ProtonDrive`)
   - **Cache size** cap (default 5 GiB)
3. After saving, the tray icon appears and your Drive is mounted.
   Open your file manager and look for **ProtonDrive** in the sidebar.

Credentials are stored in your **system keyring** (GNOME Keyring / KWallet),
not in plain text on disk.

---

## Building from source

System dependencies (Arch names; use the equivalents on your distro):

```
rust >= 1.88   gtk4   libadwaita   fuse3   libsecret   openssl   pkgconf   go
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
  protondrive-bridge   thin Rust wrapper around the official Proton Drive Go SDK
  protondrive-fuse     FUSE filesystem implementation (binary `protondrive-fs`)
  protondrive-ui       GTK4 + libadwaita setup/settings UI + tray
                       (binary `protondrive` — the user-facing app)
  protondrive-cli      headless CLI for scripting / debugging
                       (binary `protondrive-cli`)
packaging/
  deb/  rpm/  arch/  flatpak/  appimage/   per-format build assets
```

---

## Releases (for maintainers)

Push a tag of the form **`vX.Y.Z`** (strict semver, three numeric components,
e.g. `v0.1.0`, `v1.2.3`) to trigger the release workflow. CI rewrites the
workspace version automatically — you do **not** need to bump `Cargo.toml`
yourself. Any tag not matching `vX.Y.Z` is ignored.

```bash
git tag v0.1.0
git push origin v0.1.0
```

---

## License

GPL-3.0-or-later.
