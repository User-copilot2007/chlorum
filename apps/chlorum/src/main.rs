// Chlorum: the shell, the login screen and Library in one program.
//
//   chlorum shell        Home, system menu and app layouts (runs the session)
//   chlorum msg <what>   tell the running shell about a key (sent by niri)
//   chlorum greeter      the login screen
//   chlorum library      Library, the file collections
//   chlorum pick …       Library as the open/save window apps use
//   chlorum portal       serves that window to apps (xdg-desktop-portal)
//   chlorum run <id> <c> starts an app with only what it may use
//   chlorum pointer      move the mouse pointer with the keyboard (Super+M)
//   chlorum mouse [what] the numpad mouse (service; niri sends it the keys)
//   chlorum idle         turns the screen off and locks when nobody is there
//   chlorum work …       Workshop: sealed programming projects (also `work`)
//   chlorum wallpaper    the live wallpaper
//   chlorum lock         the lock screen
//   chlorum setupd       first-start helper (root, until the first profile exists)

mod apps;
mod ask;
mod atom;
mod drives;
mod greeter;
mod idle;
mod keyd;
mod layout;
mod library;
mod lock;
mod lockscreen;
mod niri;
mod notify;
mod pointer;
mod portal;
mod progress;
mod sandbox;
mod setupd;
mod shell;
mod system;
mod ui;
mod wallpaper;
mod workshop;

fn main() {
    // With vsync, the GPU driver may wait for the compositor before handing
    // out the next frame. niri stops asking a surface for frames while it is
    // covered, and on NVIDIA that wait never ends: one covered surface froze
    // the whole shell (keys, clock, half-open menu). Without vsync the frame
    // is ready at once; niri still shows it on the next refresh, so nothing
    // tears. Someone who wants it back can set ICED_PRESENT_MODE.
    if std::env::var_os("ICED_PRESENT_MODE").is_none() {
        // SAFETY: nothing else runs yet; no thread can read the environment.
        unsafe { std::env::set_var("ICED_PRESENT_MODE", "no_vsync") };
    }
    let args: Vec<String> = std::env::args().collect();
    // `work …` is Workshop, the same program under another name
    if args.first().and_then(|a| std::path::Path::new(a).file_name()).is_some_and(|n| n == "work") {
        if let Err(e) = workshop::cli(&args[1..]) {
            eprintln!("work: {e}");
            std::process::exit(1);
        }
        return;
    }
    let what = args.get(1).map(String::as_str).unwrap_or("shell");
    let result = match what {
        "shell" => shell::run().map_err(|e| e.to_string()),
        "greeter" => greeter::run().map_err(|e| e.to_string()),
        "library" => library::run().map_err(|e| e.to_string()),
        "pick" => library::pick_cli(&args[2..]),
        "portal" => portal::run(),
        "pointer" => pointer::run(),
        "mouse" => pointer::mouse_cli(&args[2..]),
        "idle" => idle::run(),
        "work" => workshop::cli(&args[2..]),
        "update-progress" => progress::cli(&args[2..]),
        "run" => sandbox::run_cli(&args[2..]),
        "ask" => ask::run(&args[2..]),
        "wallpaper" => {
            wallpaper::run();
            Ok(())
        }
        "setupd" => setupd::run_daemon(),
        "keyd" => keyd::run_daemon(),
        "lock" => lockscreen::run().map_err(|e| e.to_string()),
        "msg" => shell::msg(args.get(2).map(String::as_str).unwrap_or("super")).map_err(|e| e.to_string()),
        other => Err(format!("unknown command: {other}")),
    };
    if let Err(e) = result {
        eprintln!("chlorum: {e}");
        std::process::exit(1);
    }
}
