//! StatusNotifierItem (system tray) icon. `ksni` speaks the freedesktop SNI
//! protocol natively, so this works on KDE Plasma, Budgie, Cinnamon, and
//! GNOME (with the AppIndicator extension installed).

use anyhow::Result;
use ksni::{menu::StandardItem, MenuItem, Tray};
use protondrive_core::Daemon;

struct ProtonTray {
    daemon: Daemon,
}

impl Tray for ProtonTray {
    fn icon_name(&self) -> String {
        "folder-remote-symbolic".into()
    }
    fn title(&self) -> String {
        "Proton Drive (unofficial)".into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let root = self.daemon.config.lock().sync_root.clone();
        ksni::ToolTip {
            title: "Proton Drive (unofficial)".into(),
            description: format!("Syncing {}", root.display()),
            icon_name: "folder-remote-symbolic".into(),
            icon_pixmap: vec![],
        }
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open Proton Drive folder".into(),
                activate: Box::new(|t: &mut Self| {
                    let root = t.daemon.config.lock().sync_root.clone();
                    let _ = std::process::Command::new("xdg-open").arg(root).spawn();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open Proton Drive (web)".into(),
                activate: Box::new(|_| {
                    let _ = std::process::Command::new("xdg-open")
                        .arg("https://drive.proton.me/")
                        .spawn();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub fn run(daemon: Daemon) -> Result<()> {
    let service = ksni::TrayService::new(ProtonTray { daemon });
    service.spawn();
    std::thread::park();
    Ok(())
}
