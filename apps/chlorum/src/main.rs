// Chlorum: the shell, the login screen and Library in one program.
//
//   chlorum shell        Home, system menu and app layouts (runs the session)
//   chlorum msg <what>   tell the running shell about a key (sent by niri)
//   chlorum greeter      the login screen
//   chlorum library      Library, the file collections
//   chlorum wallpaper    the live wallpaper
//   chlorum lock         the lock screen
//   chlorum setupd       first-start helper (root, until the first profile exists)

mod apps;
mod atom;
mod greeter;
mod layout;
mod library;
mod lock;
mod lockscreen;
mod niri;
mod notify;
mod setupd;
mod shell;
mod system;
mod ui;
mod wallpaper;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let what = args.get(1).map(String::as_str).unwrap_or("shell");
    let result = match what {
        "shell" => shell::run().map_err(|e| e.to_string()),
        "greeter" => greeter::run().map_err(|e| e.to_string()),
        "library" => library::run().map_err(|e| e.to_string()),
        "wallpaper" => {
            wallpaper::run();
            Ok(())
        }
        "setupd" => setupd::run_daemon(),
        "lock" => lockscreen::run().map_err(|e| e.to_string()),
        "msg" => shell::msg(args.get(2).map(String::as_str).unwrap_or("super")).map_err(|e| e.to_string()),
        other => Err(format!("unknown command: {other}")),
    };
    if let Err(e) = result {
        eprintln!("chlorum: {e}");
        std::process::exit(1);
    }
}
