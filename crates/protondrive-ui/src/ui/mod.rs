//! GTK4 + libadwaita setup window. Implements the four-page wizard described
//! in the plan: credentials → TOTP secret → mount point/cache → progress.

use gtk4::prelude::*;
use gtk4::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use protondrive_core::Daemon;

pub fn build_main_window(app: &adw::Application, daemon: Daemon) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Proton Drive")
        .default_width(640)
        .default_height(520)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let stack = adw::ViewStack::new();
    stack.add_titled(&page_credentials(daemon.clone()), Some("creds"), "Account");
    stack.add_titled(&page_totp(daemon.clone()),       Some("totp"),  "Two-Factor");
    stack.add_titled(&page_mount(daemon.clone()),      Some("mount"), "Mount & Cache");
    stack.add_titled(&page_status(daemon.clone()),     Some("status"),"Status");

    let switcher = adw::ViewSwitcherBar::builder().stack(&stack).reveal(true).build();

    let outer = gtk4::Box::new(Orientation::Vertical, 0);
    outer.append(&stack);
    outer.append(&switcher);
    toolbar.set_content(Some(&outer));
    window.set_content(Some(&toolbar));
    window.present();
}

fn page_credentials(daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Proton account")
        .description("Your credentials are stored in the system keyring (libsecret).")
        .build();

    let email = adw::EntryRow::builder().title("Email").build();
    if let Some(e) = &daemon.config.email { email.set_text(e); }
    let password = adw::PasswordEntryRow::builder().title("Password").build();
    group.add(&email);
    group.add(&password);

    let save = gtk4::Button::with_label("Save & Sign in");
    save.add_css_class("suggested-action");
    save.set_halign(Align::End);
    let daemon_c = daemon.clone();
    let email_c = email.clone();
    let password_c = password.clone();
    save.connect_clicked(move |_| {
        let mut cfg = daemon_c.config.clone();
        let email_text = email_c.text().to_string();
        cfg.email = Some(email_text.clone());
        if let Err(e) = cfg.save(&daemon_c.paths.config_file()) {
            tracing::warn!(error=%e, "save config");
        }
        let pw = password_c.text().to_string();
        let kr = protondrive_core::keyring::Keyring::for_account(email_text);
        // Fire-and-forget: keyring is async (D-Bus). The UI should later show toast.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                if let Err(e) = kr.store(protondrive_core::keyring::Slot::Password, &pw).await {
                    tracing::warn!(error=%e, "store password");
                }
            });
        });
    });

    page_wrap(vec![group.upcast::<gtk4::Widget>(), save.upcast::<gtk4::Widget>()])
}

fn page_totp(daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Two-factor authentication")
        .description("Paste the Base32 TOTP secret (the key — not a 6-digit code) from Proton's 2FA setup screen.")
        .build();

    let secret = adw::EntryRow::builder().title("TOTP secret (Base32)").build();
    group.add(&secret);

    let preview = gtk4::Label::new(Some("current code: ——————"));
    preview.add_css_class("dim-label");
    preview.set_halign(Align::Start);

    let secret_for_preview = secret.clone();
    let preview_c = preview.clone();
    secret.connect_changed(move |row| {
        let s = row.text().to_string();
        match protondrive_core::auth::totp::current_code(&s) {
            Ok(code) => preview_c.set_text(&format!("current code: {code}")),
            Err(_)   => preview_c.set_text("current code: ——————"),
        }
        let _ = &secret_for_preview;
    });

    let save = gtk4::Button::with_label("Save TOTP secret");
    save.add_css_class("suggested-action");
    save.set_halign(Align::End);
    let daemon_c = daemon.clone();
    let secret_c = secret.clone();
    save.connect_clicked(move |_| {
        let Some(email) = daemon_c.config.email.clone() else { return; };
        let s = secret_c.text().to_string();
        if protondrive_core::auth::totp::validate_secret(&s).is_err() { return; }
        let kr = protondrive_core::keyring::Keyring::for_account(email);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let _ = kr.store(protondrive_core::keyring::Slot::TotpSecret, &s).await;
            });
        });
    });

    page_wrap(vec![group.upcast(), preview.upcast(), save.upcast()])
}

fn page_mount(daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder().title("Mount point & cache").build();
    let mount = adw::EntryRow::builder().title("Mount point").build();
    mount.set_text(daemon.config.mount_point.to_string_lossy().as_ref());
    let cache_gb = adw::SpinRow::with_range(1.0, 1024.0, 1.0);
    cache_gb.set_title("Cache size (GiB)");
    cache_gb.set_value((daemon.config.cache_max_bytes / (1024*1024*1024)).max(1) as f64);
    group.add(&mount);
    group.add(&cache_gb);

    let save = gtk4::Button::with_label("Apply");
    save.add_css_class("suggested-action");
    save.set_halign(Align::End);
    let daemon_c = daemon.clone();
    let mount_c = mount.clone();
    let cache_c = cache_gb.clone();
    save.connect_clicked(move |_| {
        let mut cfg = daemon_c.config.clone();
        cfg.mount_point = mount_c.text().to_string().into();
        cfg.cache_max_bytes = (cache_c.value() as u64) * 1024 * 1024 * 1024;
        let _ = cfg.save(&daemon_c.paths.config_file());
    });

    page_wrap(vec![group.upcast(), save.upcast()])
}

fn page_status(daemon: Daemon) -> gtk4::Widget {
    let group = adw::PreferencesGroup::builder().title("Status").build();
    let row = adw::ActionRow::builder().title("Daemon").subtitle("running").build();
    group.add(&row);

    let refresh = gtk4::Button::with_label("Refresh now");
    refresh.set_halign(Align::End);
    let daemon_c = daemon.clone();
    refresh.connect_clicked(move |_| daemon_c.sync.refresh_now());

    page_wrap(vec![group.upcast(), refresh.upcast()])
}

fn page_wrap(children: Vec<gtk4::Widget>) -> gtk4::Widget {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    let vbox = gtk4::Box::new(Orientation::Vertical, 12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    for c in children { vbox.append(&c); }
    group.add(&vbox);
    page.add(&group);
    page.upcast()
}
