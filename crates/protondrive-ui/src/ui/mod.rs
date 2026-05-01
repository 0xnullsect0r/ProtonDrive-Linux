//! GTK4 + libadwaita setup window. Four-page wizard:
//! warning → credentials → TOTP → sync folder/cache → status.

use gtk4::prelude::*;
use gtk4::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use protondrive_core::Daemon;

pub fn build_main_window(app: &adw::Application, daemon: Daemon) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Proton Drive (unofficial)")
        .default_width(720)
        .default_height(560)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let stack = adw::ViewStack::new();
    stack.add_titled_with_icon(
        &page_welcome(),
        Some("welcome"),
        "Welcome",
        "emblem-default-symbolic",
    );
    stack.add_titled_with_icon(
        &page_credentials(daemon.clone()),
        Some("creds"),
        "Account",
        "dialog-password-symbolic",
    );
    stack.add_titled_with_icon(
        &page_totp(daemon.clone()),
        Some("totp"),
        "Two-Factor",
        "system-lock-screen-symbolic",
    );
    stack.add_titled_with_icon(
        &page_folder(daemon.clone()),
        Some("folder"),
        "Sync Folder",
        "folder-symbolic",
    );
    stack.add_titled_with_icon(
        &page_status(daemon.clone()),
        Some("status"),
        "Status",
        "emblem-synchronizing-symbolic",
    );

    let switcher = adw::ViewSwitcherBar::builder()
        .stack(&stack)
        .reveal(true)
        .build();

    let outer = gtk4::Box::new(Orientation::Vertical, 0);
    outer.append(&stack);
    outer.append(&switcher);
    toolbar.set_content(Some(&outer));
    window.set_content(Some(&toolbar));
    window.present();
}

fn page_welcome() -> gtk4::Widget {
    let banner = adw::Banner::builder()
        .title("Unofficial third-party app — not affiliated with or endorsed by Proton AG.")
        .revealed(true)
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("ProtonDrive-Linux")
        .description(concat!(
            "An open-source Linux client for Proton Drive. ",
            "Your password is never stored. We keep only the salted key passphrase ",
            "(in libsecret) so the app can resume without re-asking for your password.\n\n",
            "Steps: enter your account, paste your TOTP secret if you use 2FA, ",
            "pick a sync folder, then sign in."
        ))
        .build();
    page_wrap(vec![banner.upcast(), group.upcast()])
}

fn page_credentials(daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Proton account")
        .description("Credentials are stored in the system keyring (libsecret). Your password itself is never persisted.")
        .build();

    let email = adw::EntryRow::builder().title("Email").build();
    if let Some(e) = daemon.config.lock().email.clone() {
        email.set_text(&e);
    }
    let password = adw::PasswordEntryRow::builder().title("Password").build();
    let mailbox = adw::PasswordEntryRow::builder()
        .title("Mailbox password (only for two-password mode)")
        .build();
    let totp = adw::PasswordEntryRow::builder()
        .title("TOTP secret key (Base32, optional — leave blank if no 2FA)")
        .build();
    group.add(&email);
    group.add(&password);
    group.add(&mailbox);
    group.add(&totp);

    let status = gtk4::Label::new(None);
    status.set_halign(Align::Start);
    status.add_css_class("dim-label");
    status.set_wrap(true);
    status.set_max_width_chars(60);

    let signin = gtk4::Button::with_label("Sign in");
    signin.add_css_class("suggested-action");
    signin.set_halign(Align::End);

    let daemon_c = daemon.clone();
    let email_c = email.clone();
    let password_c = password.clone();
    let mailbox_c = mailbox.clone();
    let totp_c = totp.clone();
    let status_c = status.clone();
    signin.connect_clicked(move |btn| {
        let email_text = email_c.text().to_string();
        let pw = password_c.text().to_string();
        let mb = mailbox_c.text().to_string();
        let mb_opt = (!mb.is_empty()).then_some(mb);
        let totp_secret = totp_c.text().to_string();
        let totp_secret_opt = (!totp_secret.trim().is_empty()).then(|| totp_secret.clone());

        // Generate live TOTP code from the typed secret or keyring-stored secret.
        let kr_email = email_text.clone();
        let totp_code: Option<String> = match totp_secret_opt.as_ref() {
            Some(s) => protondrive_core::auth::totp::current_code(s).ok(),
            None => std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                rt.block_on(async {
                    let kr = protondrive_core::keyring::Keyring::for_account(kr_email);
                    kr.fetch(protondrive_core::keyring::Slot::TotpSecret)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|s| protondrive_core::auth::totp::current_code(&s).ok())
                })
            })
            .join()
            .ok()
            .flatten(),
        };

        // Persist a freshly typed TOTP secret to the keyring.
        if let Some(s) = totp_secret_opt.clone() {
            let kr_store_email = email_text.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async {
                    let kr = protondrive_core::keyring::Keyring::for_account(kr_store_email);
                    let _ = kr
                        .store(protondrive_core::keyring::Slot::TotpSecret, &s)
                        .await;
                });
            });
        }

        btn.set_sensitive(false);
        status_c.set_text("Signing in…");

        let daemon_login = daemon_c.clone();
        let status_done = status_c.clone();
        let btn_done = btn.clone();
        // Channel carries Result: Ok(Option<(hv_token, methods)>) on HV, Ok(None) on success, Err on failure.
        let (tx, rx) = async_channel::bounded::<std::result::Result<Option<(String, Vec<String>)>, String>>(1);
        let email_for_hv = email_text.clone();
        let totp_secret_for_hv = totp_secret_opt.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let res = rt
                .block_on(daemon_login.login(
                    &email_text,
                    &pw,
                    mb_opt.as_deref(),
                    totp_code.as_deref(),
                ))
                .map_err(|e| e.to_string());
            let mapped = res.map(|outcome| match outcome {
                protondrive_core::LoginOutcome::Success(_) => None,
                protondrive_core::LoginOutcome::HvRequired { hv_token, methods } => {
                    Some((hv_token, methods))
                }
            });
            let _ = tx.send_blocking(mapped);
        });

        let daemon_hv = daemon_c.clone();
        glib::spawn_future_local(async move {
            if let Ok(res) = rx.recv().await {
                btn_done.set_sensitive(true);
                match res {
                    Ok(None) => status_done.set_text("Signed in. Sync starting…"),
                    Ok(Some((hv_token, methods))) => {
                        // HV required: start local captcha server and open browser.
                        status_done.set_text(
                            "Proton requires CAPTCHA verification. A browser window has been opened — please complete the challenge there. This window will update automatically.",
                        );
                        let methods_clone = methods.clone();
                        let token_clone = hv_token.clone();
                        let (tx2, rx2) = async_channel::bounded::<(String, String)>(1);
                        std::thread::spawn(move || {
                            if let Some((hv_type, solved_token)) =
                                run_hv_server(&token_clone, &methods_clone)
                            {
                                let _ = tx2.send_blocking((hv_type, solved_token));
                            }
                        });
                        // Wait for the captcha to be solved.
                        if let Ok((hv_type, solved_token)) = rx2.recv().await {
                            status_done.set_text("Captcha solved, completing sign-in…");
                            // Generate a fresh TOTP code since the original may have expired.
                            let fresh_totp = totp_secret_for_hv.as_deref()
                                .and_then(|s| protondrive_core::auth::totp::current_code(s).ok())
                                .or_else(|| {
                                    // Try keyring
                                    let kr_email2 = email_for_hv.clone();
                                    std::thread::spawn(move || {
                                        let rt = tokio::runtime::Builder::new_current_thread()
                                            .enable_all()
                                            .build()
                                            .ok()?;
                                        rt.block_on(async {
                                            let kr = protondrive_core::keyring::Keyring::for_account(kr_email2);
                                            kr.fetch(protondrive_core::keyring::Slot::TotpSecret)
                                                .await
                                                .ok()
                                                .flatten()
                                                .and_then(|s| protondrive_core::auth::totp::current_code(&s).ok())
                                        })
                                    })
                                    .join()
                                    .ok()
                                    .flatten()
                                });
                            let (tx3, rx3) = async_channel::bounded::<std::result::Result<(), String>>(1);
                            std::thread::spawn(move || {
                                let rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .unwrap();
                                let res = rt
                                    .block_on(daemon_hv.login_hv(
                                        &hv_type,
                                        &solved_token,
                                        fresh_totp.as_deref(),
                                    ))
                                    .map_err(|e| e.to_string());
                                let _ = tx3.send_blocking(res);
                            });
                            if let Ok(res) = rx3.recv().await {
                                match res {
                                    Ok(()) => status_done.set_text("Signed in. Sync starting…"),
                                    Err(e) => status_done.set_text(&format!("Sign-in failed after CAPTCHA: {e}")),
                                }
                            }
                        } else {
                            status_done.set_text("CAPTCHA verification was not completed.");
                        }
                    }
                    Err(e) => {
                        let lower = e.to_lowercase();
                        let friendly = if lower.contains("2064") {
                            "Sign-in failed: Proton rejected the request as malformed (Code 2064). \
                             Please update ProtonDrive-Linux."
                                .to_string()
                        } else if lower.contains("totp") || lower.contains("2fa") {
                            format!(
                                "Sign-in failed: 2FA code rejected. Check the TOTP secret key (Base32, no spaces). Error: {e}"
                            )
                        } else {
                            format!("Sign-in failed: {e}")
                        };
                        status_done.set_text(&friendly);
                    }
                }
            }
        });
    });

    page_wrap(vec![group.upcast(), status.upcast(), signin.upcast()])
}

/// Starts a minimal local HTTP server that serves a captcha iframe page,
/// opens the browser, and waits for the Human Verification token.
/// Returns `Some((hv_type, token))` on success.
fn run_hv_server(hv_token: &str, methods: &[String]) -> Option<(String, String)> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    let methods_str = if methods.is_empty() {
        "captcha".to_string()
    } else {
        methods.join(",")
    };

    // Build the HTML page that embeds verify.proton.me and captures the result.
    let verify_url = format!(
        "https://verify.proton.me/?methods={}&token={}&theme=dark",
        methods_str, hv_token
    );
    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Proton Human Verification</title>
<style>
  body {{ margin:0; padding:0; background:#1a1a2e; display:flex; flex-direction:column; height:100vh; }}
  h2 {{ color:#fff; font-family:sans-serif; padding:12px; margin:0; font-size:14px; background:#0e0e1a; }}
  iframe {{ flex:1; border:none; }}
</style>
</head>
<body>
<h2>Complete the verification below, then return to ProtonDrive-Linux.</h2>
<iframe id="hv" src="{verify_url}" allow="cross-origin-isolated"></iframe>
<script>
window.addEventListener("message", function(e) {{
  var d = e.data;
  if (!d) return;
  // Proton's verify page posts several message shapes:
  //   {{ type:"pm_captcha", token:"..." }}
  //   {{ type:"human-verification", token:"...", tokenType:"captcha" }}
  var token = d.token || d.captcha_token || "";
  var tokenType = d.tokenType || d.type || "captcha";
  if (tokenType === "pm_captcha") tokenType = "captcha";
  if (token) {{
    fetch("/submit", {{
      method: "POST",
      headers: {{"Content-Type": "application/json"}},
      body: JSON.stringify({{token: token, tokenType: tokenType}})
    }}).then(function() {{
      document.body.innerHTML = "<h2 style='color:#0f0;font-family:sans-serif;padding:24px;'>Verification complete! You can close this tab.</h2>";
    }});
  }}
}});
</script>
</body>
</html>"#,
        verify_url = verify_url
    );

    // Open the browser to the local page.
    let url = format!("http://127.0.0.1:{port}/");
    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();

    // Serve HTTP — handle GET / (HTML page) and POST /submit (token receiver).
    let mut result: Option<(String, String)> = None;
    for stream in listener.incoming() {
        if let Ok(mut stream) = stream {
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");

            if first_line.starts_with("POST /submit") {
                // Extract JSON body after the blank line.
                let body = req.split("\r\n\r\n").nth(1).unwrap_or("").trim();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                    let token = v["token"].as_str().unwrap_or("").to_string();
                    let token_type = v["tokenType"].as_str().unwrap_or("captcha").to_string();
                    result = Some((token_type, token));
                }
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
                break;
            } else if first_line.starts_with("GET /") {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                    html.len(),
                    html
                );
                let _ = stream.write_all(resp.as_bytes());
            } else {
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        }
    }
    result
}

fn page_totp(daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Two-factor authentication")
        .description("Paste the Base32 TOTP secret (the key — not a 6-digit code) you saved when enabling 2FA on Proton.")
        .build();

    let secret = adw::EntryRow::builder()
        .title("TOTP secret (Base32)")
        .build();
    group.add(&secret);

    let preview = gtk4::Label::new(Some("current code: ——————"));
    preview.add_css_class("dim-label");
    preview.set_halign(Align::Start);

    let preview_c = preview.clone();
    secret.connect_changed(move |row| {
        let s = row.text().to_string();
        match protondrive_core::auth::totp::current_code(&s) {
            Ok(code) => preview_c.set_text(&format!("current code: {code}")),
            Err(_) => preview_c.set_text("current code: ——————"),
        }
    });

    let save = gtk4::Button::with_label("Save TOTP secret");
    save.add_css_class("suggested-action");
    save.set_halign(Align::End);
    let daemon_c = daemon.clone();
    let secret_c = secret.clone();
    save.connect_clicked(move |_| {
        let Some(email) = daemon_c.config.lock().email.clone() else {
            return;
        };
        let s = secret_c.text().to_string();
        if protondrive_core::auth::totp::validate_secret(&s).is_err() {
            return;
        }
        let kr = protondrive_core::keyring::Keyring::for_account(email);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let _ = kr
                    .store(protondrive_core::keyring::Slot::TotpSecret, &s)
                    .await;
            });
        });
    });

    page_wrap(vec![group.upcast(), preview.upcast(), save.upcast()])
}

fn page_folder(daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Sync folder & cache")
        .description("Files in this folder are kept in sync with your Proton Drive.")
        .build();
    let folder = adw::EntryRow::builder().title("Sync folder").build();
    folder.set_text(daemon.config.lock().sync_root.to_string_lossy().as_ref());
    let cache_gb = adw::SpinRow::with_range(1.0, 1024.0, 1.0);
    cache_gb.set_title("Cache size (GiB)");
    cache_gb.set_value((daemon.config.lock().cache_max_bytes / (1024 * 1024 * 1024)).max(1) as f64);
    group.add(&folder);
    group.add(&cache_gb);

    let save = gtk4::Button::with_label("Apply");
    save.add_css_class("suggested-action");
    save.set_halign(Align::End);
    let daemon_c = daemon.clone();
    let folder_c = folder.clone();
    let cache_c = cache_gb.clone();
    save.connect_clicked(move |_| {
        {
            let mut cfg = daemon_c.config.lock();
            cfg.sync_root = folder_c.text().to_string().into();
            cfg.cache_max_bytes = (cache_c.value() as u64) * 1024 * 1024 * 1024;
        }
        let _ = daemon_c.save_config();
    });

    page_wrap(vec![group.upcast(), save.upcast()])
}

fn page_status(_daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder().title("Status").build();
    let row = adw::ActionRow::builder()
        .title("Daemon")
        .subtitle("running")
        .build();
    group.add(&row);
    page_wrap(vec![group.upcast()])
}

fn page_wrap(children: Vec<gtk4::Widget>) -> gtk4::Widget {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    let vbox = gtk4::Box::new(Orientation::Vertical, 12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    for c in children {
        vbox.append(&c);
    }
    group.add(&vbox);
    page.add(&group);
    page.upcast()
}

use gtk4::glib;
