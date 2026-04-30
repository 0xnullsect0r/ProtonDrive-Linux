# ProtonDrive-Linux

An unofficial native Linux client for [Proton Drive](https://proton.me/drive),
written in Rust.

> ⚠️ **Status: early scaffolding.** The architecture, FUSE skeleton, cache, GTK
> setup UI, system tray, keyring and config layers are in place. The
> Proton-specific protocol pieces (SRP authentication, Drive REST endpoints,
> PGP key/block decryption) are stubbed and need to be filled in by porting
> from the official open-source clients at
> <https://github.com/ProtonDriveApps>.

## What it does

- Mounts your Proton Drive as a **FUSE filesystem at `/mnt/ProtonDrive`** so
  it appears as a device in your file manager.
- Aggressive **local cache** with content-addressed blob store + SQLite
  metadata: listings are instant, files are not re-fetched on every read.
- **"Always available offline"** pinning per file or per folder.
- Polls Proton for changes every **20 seconds**, with a manual *Refresh now*
  in the system tray.
- **GTK 4 + libadwaita** setup window; **system tray** icon (StatusNotifierItem
  via `ksni`) that works on **GNOME, KDE Plasma, Budgie, and Cinnamon**.
- Credentials and TOTP secret stored in your **system keyring** (libsecret).

## Workspace layout

```
crates/
  protondrive-core   library: auth, API client, crypto, cache, sync, types
  protondrive-fuse   FUSE filesystem implementation (binary `protondrive-fs`)
  protondrive-ui     GTK4 + libadwaita setup/settings UI + tray
                     (binary `protondrive` — the user-facing app)
  protondrive-cli    headless CLI for scripting / debugging
                     (binary `protondrive-cli`)
```

## Build

System packages required (Arch names; equivalents exist on Debian/Fedora):

```
gtk4 libadwaita fuse3 libsecret openssl pkgconf
```

Then:

```
cargo build --release
```

## Run

```
# First, headless setup (or use the GTK UI; same effect)
./target/release/protondrive

# The setup wizard will ask for:
#   - Proton email + password
#   - TOTP secret (the Base32 *key*, not a 6-digit code)
#   - Sync folder (default ~/ProtonDrive)
#   - Cache size cap (default 5 GiB)
```

`~/ProtonDrive` is the default sync folder; you can change it in the
setup wizard. Files added/edited there are propagated to Proton Drive
and vice-versa.

## Releases

Push a tag of the form **`vX.Y.Z`** (strict semver, three numeric components,
e.g. `v0.1.0`, `v1.2.3`) to trigger the release workflow. The tag's version
must match `version` in the root `Cargo.toml`. Any other tag shape is
ignored by CI.

```
git tag v0.1.0
git push origin v0.1.0
```

## License

GPL-3.0-or-later.
