use dbus::{blocking::Connection, channel::Channel, message::MatchRule};

// use dbus::arg::{RefArg, Variant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let cmd_out = Command::new("ibus").arg("address").output()?;

    let address = String::from_utf8(cmd_out.stdout)?.trim_end().to_string();

    let conn: Connection = Channel::open_private(&address)?.into();

    let proxy = conn.with_proxy(
        "org.freedesktop.IBus",
        "/org/freedesktop/IBus",
        std::time::Duration::from_millis(500),
    );

    // let (res,): (Variant<Box<dyn RefArg>>,) =
    //     proxy.method_call("org.freedesktop.IBus", "GetGlobalEngine", ())?;

    // println!("{:?}", res.0);

    // let signature = res.0.signature();
    // println!("base signature: {signature}");

    // for i in res.as_iter().unwrap() {
    //     println!("{i:?}");
    // }

    // let ime_status = res
    //     .0
    //     .as_iter()
    //     .unwrap()
    //     .nth(2)
    //     .unwrap()
    //     .as_str()
    //     .unwrap()
    //     .to_owned();

    // println!("ime_status: {ime_status}");

    let signal_ml = MatchRule::new_signal("org.freedesktop.IBus", "GlobalEngineChanged");

    let _token = proxy.match_start(
        signal_ml,
        true,
        Box::new(|message, _| {
            let engine_name: String = message.read1().unwrap();
            println!("{engine_name}");

            true
        }),
    );

    loop {
        conn.process(std::time::Duration::from_millis(1000))?;
    }
}
