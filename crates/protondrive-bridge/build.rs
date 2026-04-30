use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let go_src = PathBuf::from(&crate_dir).join("go");

    println!("cargo:rerun-if-changed=go/bridge.go");
    println!("cargo:rerun-if-changed=go/go.mod");
    println!("cargo:rerun-if-changed=go/go.sum");

    let go = which::which("go").expect(
        "`go` toolchain is required to build protondrive-bridge. \
         Install Go 1.22+ (e.g. `sudo pacman -S go` / `apt install golang-go`).",
    );

    let lib_name = "protonbridge";
    let so_name = format!("lib{lib_name}.so");
    let so_path = out_dir.join(&so_name);

    let status = Command::new(&go)
        .args([
            "build",
            "-buildmode=c-shared",
            "-trimpath",
            "-ldflags=-s -w",
            "-o",
        ])
        .arg(&so_path)
        .arg(".")
        .current_dir(&go_src)
        .env("CGO_ENABLED", "1")
        .status()
        .expect("failed to invoke `go build`");

    assert!(status.success(), "go build failed (status {status})");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=dylib={lib_name}");
    // Embed an rpath relative to the executable so the .so can be
    // shipped next to the binary in distro packages.
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    // Re-export the .so location so downstream consumers (release
    // packaging) can find it.
    println!("cargo:bridge_so={}", so_path.display());
}
