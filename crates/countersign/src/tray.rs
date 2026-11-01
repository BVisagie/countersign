//! A tray icon for bars that host StatusNotifierItems, Omarchy's included.
//!
//! Left-click opens Countersign; the secondary click opens quick access.
//! The padlock shows whether the vault is open.

use std::rc::Rc;

use countersign_ipc::{ClipboardEvent, Request, Status, VaultSnapshot, VaultState};
use gtk::gio::prelude::*;
use gtk::glib;
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{MenuItem, ToolTip, Tray};

use crate::app::{App, View};

#[derive(Debug, Clone, Copy)]
enum Action {
    Open,
    Quick,
    Lock,
    Quit,
}

struct CountersignTray {
    unlocked: bool,
    actions: async_channel::Sender<Action>,
}

impl CountersignTray {
    fn item(label: &str, enabled: bool, action: Action) -> MenuItem<Self> {
        StandardItem {
            label: label.into(),
            enabled,
            activate: Box::new(move |tray: &mut Self| {
                let _ = tray.actions.try_send(action);
            }),
            ..Default::default()
        }
        .into()
    }
}

impl Tray for CountersignTray {
    fn id(&self) -> String {
        "countersign".into()
    }

    fn title(&self) -> String {
        "Countersign".into()
    }

    fn icon_name(&self) -> String {
        if self.unlocked { "changes-allow-symbolic" } else { "changes-prevent-symbolic" }.into()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Countersign".into(),
            description: if self.unlocked { "Vault unlocked" } else { "Vault locked" }.into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.actions.try_send(Action::Open);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.actions.try_send(Action::Quick);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            Self::item("Open Countersign", true, Action::Open),
            Self::item("Quick access", true, Action::Quick),
            MenuItem::Separator,
            Self::item("Lock", self.unlocked, Action::Lock),
            MenuItem::Separator,
            Self::item("Quit", true, Action::Quit),
        ]
    }

    // Keep running when no tray host is present yet; one may start later.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        true
    }
}

pub struct TrayIcon {
    handle: Handle<CountersignTray>,
}

impl View for TrayIcon {
    fn status_changed(&self, status: &Status) {
        let unlocked = status.vault == VaultState::Unlocked;
        self.handle.update(|tray| tray.unlocked = unlocked);
    }

    fn vault_changed(&self, _vault: &Rc<VaultSnapshot>) {}

    fn clipboard(&self, _event: &ClipboardEvent) {}
}

/// Starts the tray icon. Returns `None` if the session bus is unavailable.
pub fn start(app: &Rc<App>) -> Option<Rc<TrayIcon>> {
    let (actions, incoming) = async_channel::unbounded();
    let unlocked = app.status.borrow().vault == VaultState::Unlocked;
    // The bar's tray host may start after Countersign; register anyway.
    let handle = match (CountersignTray { unlocked, actions }).assume_sni_available(true).spawn() {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("countersign: no tray icon: {e}");
            return None;
        }
    };
    let weak = Rc::downgrade(app);
    glib::spawn_future_local(async move {
        while let Ok(action) = incoming.recv().await {
            let Some(app) = weak.upgrade() else { break };
            match action {
                Action::Open => app.show_main(),
                Action::Quick => app.toggle_quick(),
                Action::Lock => app.fire(Request::Lock),
                Action::Quit => app.gtk.activate_action("quit", None),
            }
        }
    });
    let icon = Rc::new(TrayIcon { handle });
    app.add_view(Rc::downgrade(&(icon.clone() as Rc<dyn View>)));
    Some(icon)
}
