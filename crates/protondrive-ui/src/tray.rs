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
        "Proton Drive".into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Proton Drive".into(),
            description: format!("Mounted at {}", self.daemon.config.mount_point.display()),
            icon_name: "folder-remote-symbolic".into(),
            icon_pixmap: vec![],
        }
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open Proton Drive folder".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = std::process::Command::new("xdg-open")
                        .arg(&t.daemon.config.mount_point)
                        .spawn();
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Refresh now".into(),
                activate: Box::new(|t: &mut Self| t.daemon.sync.refresh_now()),
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
    // Spawns its own background thread; blocks here until the bus connection drops.
    service.spawn();
    std::thread::park();
    Ok(())
}
