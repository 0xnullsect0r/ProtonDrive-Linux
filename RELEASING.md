# Release process

Releases are fully automated. To cut a release:

```bash
git tag -a v0.1.0 -m "Release 0.1.0"
git push origin v0.1.0
```

The `release.yml` workflow then:

1. Builds release binaries for `x86_64-unknown-linux-gnu`.
2. Produces a source tarball + SHA-256.
3. Builds packages in parallel:
   - **`.deb`** (Debian, Ubuntu, Mint, Pop!_OS) via `cargo-deb`
   - **`.rpm`** (Fedora, openSUSE, RHEL, CachyOS-RPM if any) via `rpmbuild` in a Fedora 40 container
   - **Arch `PKGBUILD` + `.SRCINFO`** (Arch, CachyOS, EndeavourOS, Manjaro)
   - **`.AppImage`** (works on any glibc-based distro) via `linuxdeploy` + GTK plugin
   - **`.flatpak`** via `flatpak-builder` (GNOME 46 runtime)
4. Pushes the rendered `PKGBUILD`/`.SRCINFO` to the AUR (`protondrive-linux`)
   using `KSXGitHub/github-actions-deploy-aur`.
5. Creates a GitHub Release tagged `v<version>` with every artifact + a
   `SHA256SUMS` file attached.

## Required repository secrets

Configure these in **Settings → Secrets and variables → Actions**:

| Secret                  | What it is                                                          |
|-------------------------|---------------------------------------------------------------------|
| `AUR_USERNAME`          | Your AUR account username                                           |
| `AUR_EMAIL`             | The email associated with your AUR SSH key                          |
| `AUR_SSH_PRIVATE_KEY`   | The private half of an SSH key registered to your AUR account       |

Without these the AUR-publish job will fail; the rest of the release will still
succeed and the rendered `PKGBUILD` will be in the release artifacts so you
can push manually.

## Local dry-run

You can trigger the workflow without a tag from the **Actions** tab via
"Run workflow", supplying a version. AUR push is automatically skipped on
manual runs.
