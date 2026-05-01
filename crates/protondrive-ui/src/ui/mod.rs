//! GTK4 + libadwaita main window.
//!
//! Three-tab layout inspired by Proton Drive for Windows:
//!   Syncing  |  Account  |  Settings
//!
//! * **Syncing** — live status badge, animated progress bar, recent file
//!   activity list, and a "Sync Now" button.
//! * **Account** — shows login state; credential form when signed out.
//! * **Settings** — sync folder, cache quota, and TOTP secret.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use gtk4::prelude::*;
use gtk4::{Align, Orientation};
use libadwaita as adw;
use libadwaita::prelude::*;
use protondrive_core::Daemon;
use protondrive_sync::{Direction, SyncEvent};

use crate::sync::SyncController;

pub fn build_main_window(app: &adw::Application, daemon: Daemon, sync_ctrl: SyncController) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Proton Drive")
        .default_width(480)
        .default_height(620)
        .resizable(false)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&gtk4::Label::new(Some("Proton Drive")))
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let stack = adw::ViewStack::new();

    // ── Syncing tab ────────────────────────────────────────────────
    let (sync_page, sync_state) = page_syncing(daemon.clone(), sync_ctrl.clone());
    stack.add_titled_with_icon(
        &sync_page,
        Some("syncing"),
        "Syncing",
        "emblem-synchronizing-symbolic",
    );

    // ── Account tab ────────────────────────────────────────────────
    let account_page = page_account(daemon.clone(), sync_ctrl.clone());
    stack.add_titled_with_icon(
        &account_page,
        Some("account"),
        "Account",
        "system-users-symbolic",
    );

    // ── Settings tab ───────────────────────────────────────────────
    stack.add_titled_with_icon(
        &page_settings(daemon.clone()),
        Some("settings"),
        "Settings",
        "preferences-system-symbolic",
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

    // Start on the Syncing tab (or Account if not logged in yet).
    if daemon.is_logged_in() {
        stack.set_visible_child_name("syncing");
    } else {
        stack.set_visible_child_name("account");
    }

    // Consume the async-channel receiver and wire it to sync state updates.
    if let Some(rx) = sync_ctrl.take_ui_rx() {
        let ss = sync_state.clone();
        glib::spawn_future_local(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => apply_sync_event(ev, &ss),
                    Err(_) => break,
                }
            }
        });
    }

    // 1-second timer: refresh "X ago" labels and account login badge.
    let ss_timer = sync_state.clone();
    let daemon_timer = daemon.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        refresh_time_labels(&ss_timer);
        refresh_account_badge(&daemon_timer);
        glib::ControlFlow::Continue
    });

    window.present();
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared sync state (lives on the GTK main thread).
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
struct FileActivity {
    rel: String,
    direction: Direction,
    at: DateTime<Utc>,
}

#[derive(Clone)]
struct SyncState {
    inner: Rc<RefCell<SyncStateInner>>,
}

struct SyncStateInner {
    status: Status,
    last_idle: Option<DateTime<Utc>>,
    recent: VecDeque<FileActivity>,

    // GTK widget handles for live updates.
    status_label: gtk4::Label,
    status_icon: gtk4::Image,
    progress: gtk4::ProgressBar,
    last_synced_label: gtk4::Label,
    file_list: gtk4::ListBox,
    empty_label: gtk4::Label,
}

#[derive(Clone, PartialEq)]
enum Status {
    NotRunning,
    Starting,
    Idle,
    Busy(usize),
    Error(String),
}

impl SyncState {
    fn new(
        status_label: gtk4::Label,
        status_icon: gtk4::Image,
        progress: gtk4::ProgressBar,
        last_synced_label: gtk4::Label,
        file_list: gtk4::ListBox,
        empty_label: gtk4::Label,
    ) -> Self {
        Self {
            inner: Rc::new(RefCell::new(SyncStateInner {
                status: Status::NotRunning,
                last_idle: None,
                recent: VecDeque::with_capacity(50),
                status_label,
                status_icon,
                progress,
                last_synced_label,
                file_list,
                empty_label,
            })),
        }
    }
}

fn apply_sync_event(ev: SyncEvent, ss: &SyncState) {
    let mut s = ss.inner.borrow_mut();
    match ev {
        SyncEvent::Started => {
            s.status = Status::Starting;
            s.status_label.set_label("Connecting…");
            s.status_icon
                .set_icon_name(Some("content-loading-symbolic"));
            s.progress.set_visible(true);
            s.progress.pulse();
        }
        SyncEvent::Busy { queue } => {
            s.status = Status::Busy(queue);
            s.status_label.set_label(&format!("Syncing {queue} items…"));
            s.status_icon
                .set_icon_name(Some("emblem-synchronizing-symbolic"));
            s.progress.set_visible(true);
            s.progress.pulse();
        }
        SyncEvent::Idle { at } => {
            s.status = Status::Idle;
            s.last_idle = Some(at);
            s.status_label.set_label("Up to date");
            s.status_icon
                .set_icon_name(Some("emblem-default-symbolic"));
            s.progress.set_visible(false);
            s.last_synced_label
                .set_label(&format!("Last synced: {}", fmt_age(at)));
        }
        SyncEvent::Synced { rel, direction, at } => {
            let mut s = s; // reborrow
            if s.recent.len() >= 50 {
                s.recent.pop_back();
                // Remove last row from list.
                let last = s.file_list.last_child();
                if let Some(w) = last {
                    s.file_list.remove(&w);
                }
            }
            let row = make_file_row(&rel, &direction, at);
            s.file_list.prepend(&row);
            s.recent.push_front(FileActivity { rel, direction, at });
            s.empty_label.set_visible(false);
        }
        SyncEvent::Error { message } => {
            s.status = Status::Error(message.clone());
            s.status_label.set_label(&format!("Error: {message}"));
            s.status_icon
                .set_icon_name(Some("dialog-error-symbolic"));
            s.progress.set_visible(false);
        }
    }
}

fn refresh_time_labels(ss: &SyncState) {
    let s = ss.inner.borrow();
    if let Some(at) = s.last_idle {
        s.last_synced_label
            .set_label(&format!("Last synced: {}", fmt_age(at)));
    }
    // Pulse progress bar while busy/starting.
    if matches!(s.status, Status::Busy(_) | Status::Starting) {
        s.progress.pulse();
    }
}

// Widgets on the Account page that need refreshing on the 1-second timer.
thread_local! {
    static ACCOUNT_WIDGETS: RefCell<Option<AccountWidgets>> = const { RefCell::new(None) };
}

struct AccountWidgets {
    daemon: Daemon,
    logged_in_box: gtk4::Box,
    email_label: gtk4::Label,
    login_box: gtk4::Box,
}

fn refresh_account_badge(daemon: &Daemon) {
    ACCOUNT_WIDGETS.with(|cell| {
        if let Some(aw) = cell.borrow().as_ref() {
            let logged_in = aw.daemon.is_logged_in();
            aw.logged_in_box.set_visible(logged_in);
            aw.login_box.set_visible(!logged_in);
            if logged_in {
                if let Some(email) = aw.daemon.email() {
                    aw.email_label
                        .set_label(&format!("Signed in as {email}"));
                }
            }
        }
        if daemon.is_logged_in() {
            if let Some(aw) = cell.borrow().as_ref() {
                if let Some(email) = aw.daemon.email() {
                    aw.email_label
                        .set_label(&format!("Signed in as {email}"));
                }
                aw.logged_in_box.set_visible(true);
                aw.login_box.set_visible(false);
            }
        }
    });
}

fn fmt_age(at: DateTime<Utc>) -> String {
    let secs = (Utc::now() - at).num_seconds().max(0);
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86400 {
        format!("{} hr ago", secs / 3600)
    } else {
        format!("{} days ago", secs / 86400)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Page: Syncing
// ─────────────────────────────────────────────────────────────────────────────

fn page_syncing(daemon: Daemon, sync_ctrl: SyncController) -> (gtk4::Widget, SyncState) {
    // ── Status row ─────────────────────────────────────────────────
    let status_icon = gtk4::Image::from_icon_name("emblem-default-symbolic");
    status_icon.set_icon_size(gtk4::IconSize::Large);

    let status_label = gtk4::Label::new(Some(if daemon.is_logged_in() {
        "Up to date"
    } else {
        "Sign in to start syncing"
    }));
    status_label.add_css_class("title-2");
    status_label.set_halign(Align::Start);

    let last_synced_label = gtk4::Label::new(Some("Last synced: never"));
    last_synced_label.add_css_class("dim-label");
    last_synced_label.set_halign(Align::Start);

    let status_vbox = gtk4::Box::new(Orientation::Vertical, 4);
    status_vbox.append(&status_label);
    status_vbox.append(&last_synced_label);
    status_vbox.set_hexpand(true);

    let sync_now_btn = gtk4::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Sync Now")
        .valign(Align::Center)
        .build();
    sync_now_btn.add_css_class("circular");
    sync_now_btn.add_css_class("flat");

    let status_row = gtk4::Box::new(Orientation::Horizontal, 12);
    status_row.set_margin_top(16);
    status_row.set_margin_start(16);
    status_row.set_margin_end(16);
    status_row.append(&status_icon);
    status_row.append(&status_vbox);
    status_row.append(&sync_now_btn);

    // ── Progress bar ───────────────────────────────────────────────
    let progress = gtk4::ProgressBar::new();
    progress.set_margin_start(16);
    progress.set_margin_end(16);
    progress.set_visible(false);
    progress.add_css_class("osd");

    // ── Recent Activity ────────────────────────────────────────────
    let section_label = gtk4::Label::new(Some("Recent Activity"));
    section_label.add_css_class("heading");
    section_label.set_halign(Align::Start);
    section_label.set_margin_top(8);
    section_label.set_margin_start(16);

    let empty_label = gtk4::Label::new(Some("No recent activity"));
    empty_label.add_css_class("dim-label");
    empty_label.set_halign(Align::Center);
    empty_label.set_margin_top(24);
    empty_label.set_margin_bottom(24);
    let empty_visible = !daemon.is_logged_in()
        || !sync_ctrl.is_running();
    empty_label.set_visible(empty_visible);

    let file_list = gtk4::ListBox::new();
    file_list.set_selection_mode(gtk4::SelectionMode::None);
    file_list.add_css_class("boxed-list");
    file_list.set_margin_start(16);
    file_list.set_margin_end(16);

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .min_content_height(200)
        .max_content_height(350)
        .child(&file_list)
        .build();
    scroll.set_margin_start(0);
    scroll.set_margin_end(0);

    // Create shared sync state.
    let sync_state = SyncState::new(
        status_label,
        status_icon,
        progress.clone(),
        last_synced_label,
        file_list.clone(),
        empty_label.clone(),
    );

    // "Sync Now" wires.
    let sc = sync_ctrl.clone();
    let d = daemon.clone();
    sync_now_btn.connect_clicked(move |_| {
        sc.restart(&d);
    });

    // Layout.
    let vbox = gtk4::Box::new(Orientation::Vertical, 8);
    vbox.set_vexpand(true);
    vbox.append(&status_row);
    vbox.append(&progress);
    vbox.append(&section_label);
    vbox.append(&empty_label);
    vbox.append(&scroll);
    vbox.set_margin_bottom(16);

    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    group.set_margin_top(0);

    // Wrap in a plain box (not a group) to avoid extra chrome.
    page.add(&group);
    // We can't easily add arbitrary widgets to adw::PreferencesPage inside a
    // group so we use a Clamp for nice centering.
    let clamp = adw::Clamp::builder()
        .maximum_size(700)
        .child(&vbox)
        .build();

    let scroll_outer = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&clamp)
        .vexpand(true)
        .build();

    (scroll_outer.upcast(), sync_state)
}

fn make_file_row(rel: &str, direction: &Direction, at: DateTime<Utc>) -> gtk4::ListBoxRow {
    let icon_name = match direction {
        Direction::Up => "go-up-symbolic",
        Direction::Down => "go-down-symbolic",
    };
    let icon = gtk4::Image::from_icon_name(icon_name);
    icon.set_margin_end(8);

    let file_icon = gtk4::Image::from_icon_name("text-x-generic-symbolic");
    file_icon.set_margin_end(8);

    // Show only the filename portion in the label for brevity.
    let display_name = rel.rsplit('/').next().unwrap_or(rel);
    let name_label = gtk4::Label::new(Some(display_name));
    name_label.set_halign(Align::Start);
    name_label.set_hexpand(true);
    name_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    name_label.set_max_width_chars(40);

    let path_label = gtk4::Label::new(Some(rel));
    path_label.add_css_class("caption");
    path_label.add_css_class("dim-label");
    path_label.set_halign(Align::Start);
    path_label.set_ellipsize(gtk4::pango::EllipsizeMode::Start);

    let name_vbox = gtk4::Box::new(Orientation::Vertical, 2);
    name_vbox.append(&name_label);
    name_vbox.append(&path_label);
    name_vbox.set_hexpand(true);
    name_vbox.set_valign(Align::Center);

    let time_label = gtk4::Label::new(Some(&fmt_age(at)));
    time_label.add_css_class("caption");
    time_label.add_css_class("dim-label");
    time_label.set_valign(Align::Center);

    let row_box = gtk4::Box::new(Orientation::Horizontal, 4);
    row_box.set_margin_top(8);
    row_box.set_margin_bottom(8);
    row_box.set_margin_start(12);
    row_box.set_margin_end(12);
    row_box.append(&file_icon);
    row_box.append(&name_vbox);
    row_box.append(&icon);
    row_box.append(&time_label);

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row.set_activatable(false);
    row
}

// ─────────────────────────────────────────────────────────────────────────────
// Page: Account
// ─────────────────────────────────────────────────────────────────────────────

fn page_account(daemon: Daemon, sync_ctrl: SyncController) -> gtk4::Widget {
    // ── "Logged in" view ───────────────────────────────────────────
    let email_label = gtk4::Label::new(Some(
        &daemon
            .email()
            .map(|e| format!("Signed in as {e}"))
            .unwrap_or_else(|| "Signed in".into()),
    ));
    email_label.add_css_class("title-3");
    email_label.set_halign(Align::Start);
    email_label.set_wrap(true);

    let sync_status_row = adw::ActionRow::builder()
        .title("Sync")
        .subtitle(if sync_ctrl.is_running() {
            "Active"
        } else {
            "Not running"
        })
        .build();
    let sync_status_icon = gtk4::Image::from_icon_name("emblem-default-symbolic");
    sync_status_row.add_prefix(&sync_status_icon);

    let signout_btn = gtk4::Button::with_label("Sign Out");
    signout_btn.add_css_class("destructive-action");
    signout_btn.set_halign(Align::Start);
    signout_btn.set_margin_top(12);

    let logged_in_group = adw::PreferencesGroup::builder()
        .title("Proton Account")
        .description("You are signed in to Proton Drive.")
        .build();
    let li_vbox = gtk4::Box::new(Orientation::Vertical, 8);
    li_vbox.set_margin_top(8);
    li_vbox.append(&email_label);
    li_vbox.append(&sync_status_row);
    li_vbox.append(&signout_btn);
    logged_in_group.add(&li_vbox);

    let logged_in_box = gtk4::Box::new(Orientation::Vertical, 0);
    logged_in_box.append(&logged_in_group);
    logged_in_box.set_visible(daemon.is_logged_in());

    // ── "Sign in" form ─────────────────────────────────────────────
    let form_group = adw::PreferencesGroup::builder()
        .title("Proton Account")
        .description(
            "Credentials are stored in the system keyring. Your password is never persisted.",
        )
        .build();

    let email_entry = adw::EntryRow::builder().title("Email").build();
    if let Some(e) = daemon.config.lock().email.clone() {
        email_entry.set_text(&e);
    }
    let password_entry = adw::PasswordEntryRow::builder().title("Password").build();
    let mailbox_entry = adw::PasswordEntryRow::builder()
        .title("Mailbox password (only for two-password mode)")
        .build();
    let totp_entry = adw::PasswordEntryRow::builder()
        .title("TOTP secret key (Base32, optional)")
        .build();

    form_group.add(&email_entry);
    form_group.add(&password_entry);
    form_group.add(&mailbox_entry);
    form_group.add(&totp_entry);

    let status_label = gtk4::Label::new(None);
    status_label.set_halign(Align::Start);
    status_label.add_css_class("dim-label");
    status_label.set_wrap(true);
    status_label.set_max_width_chars(60);

    let signin_btn = gtk4::Button::with_label("Sign In");
    signin_btn.add_css_class("suggested-action");
    signin_btn.set_halign(Align::End);

    let login_box = gtk4::Box::new(Orientation::Vertical, 12);
    login_box.append(&form_group);
    login_box.append(&status_label);
    login_box.append(&signin_btn);
    login_box.set_visible(!daemon.is_logged_in());

    // Wire up account widget refresh.
    ACCOUNT_WIDGETS.with(|cell| {
        *cell.borrow_mut() = Some(AccountWidgets {
            daemon: daemon.clone(),
            logged_in_box: logged_in_box.clone(),
            email_label: email_label.clone(),
            login_box: login_box.clone(),
        });
    });

    // Sign-out handler.
    {
        let daemon_so = daemon.clone();
        let sc_so = sync_ctrl.clone();
        let li_box = logged_in_box.clone();
        let lo_box = login_box.clone();
        signout_btn.connect_clicked(move |_| {
            sc_so.stop();
            let d = daemon_so.clone();
            let li = li_box.clone();
            let lo = lo_box.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _ = rt.block_on(d.logout());
            });
            li.set_visible(false);
            lo.set_visible(true);
        });
    }

    // Sign-in handler (same logic as before, condensed).
    wire_signin_button(
        signin_btn,
        status_label.clone(),
        email_entry.clone(),
        password_entry.clone(),
        mailbox_entry.clone(),
        totp_entry.clone(),
        daemon.clone(),
        sync_ctrl.clone(),
        logged_in_box.clone(),
        login_box.clone(),
        email_label.clone(),
    );

    let outer = gtk4::Box::new(Orientation::Vertical, 16);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(16);
    outer.set_margin_end(16);
    outer.append(&logged_in_box);
    outer.append(&login_box);

    let clamp = adw::Clamp::builder()
        .maximum_size(700)
        .child(&outer)
        .build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&clamp)
        .vexpand(true)
        .build();

    scroll.upcast()
}

#[allow(clippy::too_many_arguments)]
fn wire_signin_button(
    signin_btn: gtk4::Button,
    status_label: gtk4::Label,
    email_entry: adw::EntryRow,
    password_entry: adw::PasswordEntryRow,
    mailbox_entry: adw::PasswordEntryRow,
    totp_entry: adw::PasswordEntryRow,
    daemon: Daemon,
    sync_ctrl: SyncController,
    logged_in_box: gtk4::Box,
    login_box: gtk4::Box,
    email_label: gtk4::Label,
) {
    let daemon_c = daemon.clone();
    let email_c = email_entry.clone();
    let password_c = password_entry.clone();
    let mailbox_c = mailbox_entry.clone();
    let totp_c = totp_entry.clone();
    let status_c = status_label.clone();

    signin_btn.connect_clicked(move |btn| {
        let email_text = email_c.text().to_string();
        let pw = password_c.text().to_string();
        let mb = mailbox_c.text().to_string();
        let mb_opt = (!mb.is_empty()).then_some(mb);
        let totp_secret = totp_c.text().to_string();
        let totp_secret_opt = (!totp_secret.trim().is_empty()).then(|| totp_secret.clone());

        // Generate TOTP code from typed secret or keyring.
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

        // Persist freshly typed TOTP secret.
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
        let sync_ctrl_after = sync_ctrl.clone();
        let daemon_after = daemon_c.clone();
        let li_box = logged_in_box.clone();
        let lo_box = login_box.clone();
        let el = email_label.clone();
        let email_for_hv = email_text.clone();
        let totp_secret_for_hv = totp_secret_opt.clone();

        let (tx, rx) =
            async_channel::bounded::<std::result::Result<Option<(String, Vec<String>)>, String>>(1);
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

        glib::spawn_future_local(async move {
            if let Ok(res) = rx.recv().await {
                btn_done.set_sensitive(true);
                match res {
                    Ok(None) => {
                        // Login success.
                        on_login_success(
                            &status_done,
                            &daemon_after,
                            &sync_ctrl_after,
                            &li_box,
                            &lo_box,
                            &el,
                        );
                    }
                    Ok(Some((hv_token, methods))) => {
                        status_done.set_text(
                            "Proton requires CAPTCHA verification. A window has been opened — \
                             please complete the challenge.",
                        );
                        let methods_clone = methods.clone();
                        let token_clone = hv_token.clone();
                        let (tx2, rx2) = async_channel::bounded::<(String, String)>(1);
                        std::thread::spawn(move || {
                            if let Some((hv_type, solved)) =
                                run_hv_subprocess(&token_clone, &methods_clone)
                            {
                                let _ = tx2.send_blocking((hv_type, solved));
                            }
                        });
                        if let Ok((hv_type, solved_token)) = rx2.recv().await {
                            status_done.set_text("Captcha solved, completing sign-in…");
                            let fresh_totp = totp_secret_for_hv
                                .as_deref()
                                .and_then(|s| {
                                    protondrive_core::auth::totp::current_code(s).ok()
                                })
                                .or_else(|| {
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
                                                .and_then(|s| {
                                                    protondrive_core::auth::totp::current_code(&s)
                                                        .ok()
                                                })
                                        })
                                    })
                                    .join()
                                    .ok()
                                    .flatten()
                                });

                            let (tx3, rx3) =
                                async_channel::bounded::<std::result::Result<(), String>>(1);
                            let daemon_hv = daemon_after.clone();
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
                                    Ok(()) => on_login_success(
                                        &status_done,
                                        &daemon_after,
                                        &sync_ctrl_after,
                                        &li_box,
                                        &lo_box,
                                        &el,
                                    ),
                                    Err(e) => status_done
                                        .set_text(&format!("Sign-in failed after CAPTCHA: {e}")),
                                }
                            }
                        } else {
                            status_done.set_text("CAPTCHA was not completed.");
                        }
                    }
                    Err(e) => {
                        let lower = e.to_lowercase();
                        let friendly = if lower.contains("2064") {
                            "Sign-in failed: Proton rejected the request (Code 2064). \
                             Please update ProtonDrive-Linux."
                                .to_string()
                        } else if lower.contains("totp") || lower.contains("2fa") {
                            format!("Sign-in failed: 2FA rejected. Check the TOTP secret. Error: {e}")
                        } else {
                            format!("Sign-in failed: {e}")
                        };
                        status_done.set_text(&friendly);
                    }
                }
            }
        });
    });
}

fn on_login_success(
    status: &gtk4::Label,
    daemon: &Daemon,
    sync_ctrl: &SyncController,
    logged_in_box: &gtk4::Box,
    login_box: &gtk4::Box,
    email_label: &gtk4::Label,
) {
    status.set_text("Signed in. Sync starting…");
    if let Some(email) = daemon.email() {
        email_label.set_label(&format!("Signed in as {email}"));
    }
    logged_in_box.set_visible(true);
    login_box.set_visible(false);
    if let Err(e) = sync_ctrl.start(daemon) {
        tracing::warn!(error=%e, "sync start failed after login");
        status.set_text(&format!("Signed in, but sync failed to start: {e}"));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Page: Settings
// ─────────────────────────────────────────────────────────────────────────────

fn page_settings(daemon: Daemon) -> gtk4::Widget {
    // ── Sync folder & cache ────────────────────────────────────────
    let folder_group = adw::PreferencesGroup::builder()
        .title("Sync Folder & Cache")
        .description("Files in this folder are kept in sync with your Proton Drive.")
        .build();

    let folder_row = adw::EntryRow::builder().title("Sync folder").build();
    folder_row.set_text(
        daemon
            .config
            .lock()
            .sync_root
            .to_string_lossy()
            .as_ref(),
    );
    let cache_row = adw::SpinRow::with_range(1.0, 1024.0, 1.0);
    cache_row.set_title("Cache size (GiB)");
    cache_row.set_value(
        (daemon.config.lock().cache_max_bytes / (1024 * 1024 * 1024)).max(1) as f64,
    );
    folder_group.add(&folder_row);
    folder_group.add(&cache_row);

    let apply_btn = gtk4::Button::with_label("Apply");
    apply_btn.add_css_class("suggested-action");
    apply_btn.set_halign(Align::End);

    {
        let d = daemon.clone();
        let fr = folder_row.clone();
        let cr = cache_row.clone();
        apply_btn.connect_clicked(move |_| {
            {
                let mut cfg = d.config.lock();
                cfg.sync_root = fr.text().to_string().into();
                cfg.cache_max_bytes = (cr.value() as u64) * 1024 * 1024 * 1024;
            }
            let _ = d.save_config();
        });
    }

    // ── TOTP ───────────────────────────────────────────────────────
    let totp_group = adw::PreferencesGroup::builder()
        .title("Two-Factor Authentication")
        .description(
            "Paste the Base32 TOTP secret key (not a 6-digit code) from when you \
             enabled 2FA on your Proton account.",
        )
        .build();

    let totp_row = adw::EntryRow::builder()
        .title("TOTP secret (Base32)")
        .build();
    totp_group.add(&totp_row);

    let code_preview = gtk4::Label::new(Some("Current code: ——————"));
    code_preview.add_css_class("dim-label");
    code_preview.set_halign(Align::Start);

    let preview_c = code_preview.clone();
    totp_row.connect_changed(move |row| {
        let s = row.text().to_string();
        match protondrive_core::auth::totp::current_code(&s) {
            Ok(code) => preview_c.set_text(&format!("Current code: {code}")),
            Err(_) => preview_c.set_text("Current code: ——————"),
        }
    });

    let save_totp_btn = gtk4::Button::with_label("Save TOTP Secret");
    save_totp_btn.add_css_class("suggested-action");
    save_totp_btn.set_halign(Align::End);

    {
        let d = daemon.clone();
        let tr = totp_row.clone();
        save_totp_btn.connect_clicked(move |_| {
            let Some(email) = d.config.lock().email.clone() else {
                return;
            };
            let s = tr.text().to_string();
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
    }

    // Layout.
    let outer = gtk4::Box::new(Orientation::Vertical, 16);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(16);
    outer.set_margin_end(16);

    outer.append(&folder_group);
    outer.append(&apply_btn);
    outer.append(&totp_group);
    outer.append(&code_preview);
    outer.append(&save_totp_btn);

    let clamp = adw::Clamp::builder()
        .maximum_size(700)
        .child(&outer)
        .build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&clamp)
        .vexpand(true)
        .build();

    scroll.upcast()
}

// ─────────────────────────────────────────────────────────────────────────────
// HV helper
// ─────────────────────────────────────────────────────────────────────────────

fn run_hv_subprocess(hv_token: &str, methods: &[String]) -> Option<(String, String)> {
    let methods_str = methods
        .first()
        .cloned()
        .unwrap_or_else(|| "captcha".to_string());

    let helper = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("protondrive-hv")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("protondrive-hv"));

    let output = std::process::Command::new(&helper)
        .arg(hv_token)
        .arg(&methods_str)
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().find(|l| l.contains("token"))?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let token = v["token"].as_str()?.to_string();
    let token_type = v["tokenType"].as_str().unwrap_or("captcha").to_string();
    if token.is_empty() {
        return None;
    }
    Some((token_type, token))
}

use gtk4::glib;
