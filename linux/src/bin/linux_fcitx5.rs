use dbus::blocking::{SyncConnection, stdintf::org_freedesktop_dbus::Properties};
use dbus::message::MatchRule;

use std::sync::mpsc::sync_channel;
use std::time::Duration;

fn main() -> Result<(), dbus::Error> {
    let conn = SyncConnection::new_session()?;

    let notifier_watcher_proxy = conn.with_proxy(
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        Duration::from_millis(500),
    );

    let notifier_items: Vec<String> = notifier_watcher_proxy.get(
        "org.kde.StatusNotifierWatcher",
        "RegisteredStatusNotifierItems",
    )?;

    let fcitx5_sni_proxy = {
        let mut fcitx5_sni_proxy = None;

        for sni_name in notifier_items.into_iter() {
            let (dest, path) = {
                let (dest, path) = sni_name.split_once("@").unwrap();
                (dest.to_owned(), path.to_owned())
            };

            let sni_proxy = conn.with_proxy(dest, path, Duration::from_millis(500));
            let sni_id: String = sni_proxy.get("org.kde.StatusNotifierItem", "Id")?;

            if sni_id.as_str() == "Fcitx" {
                fcitx5_sni_proxy = Some(sni_proxy);
            }
        }

        fcitx5_sni_proxy
    };

    let Some(fcitx5_sni_proxy) = fcitx5_sni_proxy else {
        return Ok(()); // 実際にはエラーとする
    };

    let signal_ml = MatchRule::new_signal("org.kde.StatusNotifierItem", "NewIcon");

    // タイミングの通知用
    struct GetInputMethod;

    let (sender, receiver) = sync_channel(1);

    let _token = fcitx5_sni_proxy.match_start(
        signal_ml,
        true,
        Box::new(move |_message, _| {
            let _ = sender.try_send(GetInputMethod);

            true
        }),
    )?;

    std::thread::spawn({
        let worker_conn = SyncConnection::new_session()?;

        move || -> Result<(), dbus::Error> {
            let controller_proxy = worker_conn.with_proxy(
                "org.fcitx.Fcitx5",
                "/controller",
                Duration::from_millis(500),
            );

            while let Ok(_msg) = receiver.recv() {
                let (ime_status,): (String,) = controller_proxy.method_call(
                    "org.fcitx.Fcitx.Controller1",
                    "CurrentInputMethod",
                    (),
                )?;

                println!("ime_status: {ime_status}");
            }

            Ok(())
        }
    });

    loop {
        conn.process(Duration::from_millis(1000))?;
    }
}
