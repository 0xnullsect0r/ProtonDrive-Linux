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
    stack.add_titled_with_icon(&page_welcome(), Some("welcome"), "Welcome", "emblem-default-symbolic");
    stack.add_titled_with_icon(&page_credentials(daemon.clone()), Some("creds"), "Account", "dialog-password-symbolic");
    stack.add_titled_with_icon(&page_totp(daemon.clone()), Some("totp"), "Two-Factor", "system-lock-screen-symbolic");
    stack.add_titled_with_icon(&page_folder(daemon.clone()), Some("folder"), "Sync Folder", "folder-symbolic");
    stack.add_titled_with_icon(&page_status(daemon.clone()), Some("status"), "Status", "emblem-synchronizing-symbolic");

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

        // 1) If a TOTP secret was typed in this session, generate the live code
        //    from it directly (no keyring round-trip needed).
        // 2) Otherwise, fall back to a previously-saved secret in the keyring.
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

        // Persist the freshly-typed TOTP secret to the keyring so future
        // sign-ins / token refreshes succeed without re-typing it.
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
        let (tx, rx) = async_channel::bounded::<std::result::Result<(), String>>(1);
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
            let _ = tx.send_blocking(res);
        });
        glib::spawn_future_local(async move {
            if let Ok(res) = rx.recv().await {
                btn_done.set_sensitive(true);
                match res {
                    Ok(()) => status_done.set_text("Signed in. Sync starting…"),
                    Err(e) => status_done.set_text(&format!("Sign-in failed: {e}")),
                }
            }
        });
    });

    page_wrap(vec![group.upcast(), status.upcast(), signin.upcast()])
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
