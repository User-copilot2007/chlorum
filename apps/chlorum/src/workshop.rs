//! Workshop (`work`): programming without cluttering the system.
//!
//! Every project lives in Library → Projects/<name> and is its own sealed
//! box. Its compilers and tools are listed in the project's workshop.nix
//! and come from the system's Nix store: nothing is installed on the
//! system, nothing lands in your home folder (caches stay in the project's
//! .workshop folder), and the commands run in a box that sees only the
//! project, never your personal files.
//!
//! The program itself (`work run`, `work test`) is an app like any other:
//! it may use nothing (internet, camera, sound, your files) until you allow
//! it. What it wants is written in workshop.toml (`permissions = "network"`);
//! the first time, Chlorum asks you about each one, like a phone does. The
//! answers are kept under the app "work.<name>" in Center → Apps. Getting
//! the tools and libraries ready happens first, in the tools' box, which
//! may download but never sees your files.
//!
//! Every language has the same five commands:
//!   work check    is it right? (compiler warnings as errors, linters)
//!   work format   tidy the code the standard way
//!   work test     run the tests
//!   work run      start it
//!   work build    a finished build
//! and
//!   work new <name> <c|rust|python|web>
//!   work enter    a shell inside the box
//!   work list     the projects
//!   work clean    remove the project's caches and build output

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lang {
    C,
    Rust,
    Python,
    Web,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::C, Lang::Rust, Lang::Python, Lang::Web];

    pub fn key(self) -> &'static str {
        match self {
            Lang::C => "c",
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::Web => "web",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Lang::C => "C / C++",
            Lang::Rust => "Rust",
            Lang::Python => "Python",
            Lang::Web => "Web (TypeScript)",
        }
    }

    pub fn parse(s: &str) -> Option<Lang> {
        match s.to_ascii_lowercase().as_str() {
            "c" | "c++" | "cpp" => Some(Lang::C),
            "rust" | "rs" => Some(Lang::Rust),
            "python" | "py" => Some(Lang::Python),
            "web" | "js" | "ts" | "typescript" | "javascript" | "node" => Some(Lang::Web),
            _ => None,
        }
    }

    /// The tools in the box, as Nix packages.
    fn packages(self) -> &'static str {
        match self {
            Lang::C => "gcc clang-tools meson ninja pkg-config gdb",
            Lang::Rust => "cargo rustc rustfmt clippy rust-analyzer",
            Lang::Python => "(python3.withPackages (p: [ p.pytest ])) ruff pyright uv",
            Lang::Web => "nodejs typescript prettier",
        }
    }

    /// The five commands, as shell lines run inside the box.
    pub fn command(self, what: &str, name: &str) -> Option<String> {
        let module = name.replace('-', "_");
        let c_files = "find src include tests -type f \\( -name '*.c' -o -name '*.h' -o -name '*.cpp' -o -name '*.hpp' \\) 2>/dev/null";
        let setup = "test -d build || meson setup build --warnlevel=3 --werror";
        Some(match (self, what) {
            (Lang::C, "check") => format!("{setup} && ninja -C build && {c_files} | xargs -r clang-format --dry-run --Werror"),
            (Lang::C, "format") => format!("{c_files} | xargs -r clang-format -i"),
            (Lang::C, "test") => "meson test -C build --print-errorlogs --no-rebuild".into(),
            (Lang::C, "run") => format!("./build/{name}"),
            (Lang::C, "build") => format!("test -d release || meson setup release --buildtype=release; ninja -C release && echo \"Built: release/{name}\""),
            (Lang::Rust, "check") => "cargo clippy --all-targets -- -D warnings && cargo fmt --check".into(),
            (Lang::Rust, "format") => "cargo fmt".into(),
            (Lang::Rust, "test") => "cargo test --offline -q".into(),
            (Lang::Rust, "run") => "cargo run --offline -q".into(),
            (Lang::Rust, "build") => "cargo build --release && echo \"Built: target/release/\"".into(),
            (Lang::Python, "check") => "ruff check . && ruff format --check . && pyright".into(),
            (Lang::Python, "format") => "ruff check --fix . ; ruff format .".into(),
            (Lang::Python, "test") => "python -m pytest -q".into(),
            (Lang::Python, "run") => format!("PYTHONPATH=src python -m {module}"),
            (Lang::Python, "build") => "python -m compileall -q src && echo \"Checked that every file compiles.\"".into(),
            (Lang::Web, "check") => "tsc && prettier --check .".into(),
            (Lang::Web, "format") => "prettier --write .".into(),
            (Lang::Web, "test") => "node --test 'test/**/*.test.ts'".into(),
            (Lang::Web, "run") => "node src/main.ts".into(),
            (Lang::Web, "build") => "tsc && echo \"Types are correct; node runs src/main.ts as it is.\"".into(),
            _ => return None,
        })
    }

    /// For `run` and `test`: what gets the program ready (downloads,
    /// compiling), done in the tools' box before the program starts.
    pub fn prepare(self, what: &str) -> Option<String> {
        let setup = "test -d build || meson setup build --warnlevel=3 --werror";
        match (self, what) {
            (Lang::C, "run" | "test") => Some(format!("{setup} && ninja -C build")),
            (Lang::Rust, "run") => Some("cargo build -q".into()),
            (Lang::Rust, "test") => Some("cargo test --no-run -q".into()),
            _ => None,
        }
    }
}

/// The commands that start the program itself: they run with its permissions.
pub const PROGRAM: [&str; 2] = ["run", "test"];

pub const COMMANDS: [&str; 5] = ["check", "format", "test", "run", "build"];

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn projects_dir() -> PathBuf {
    home().join("Library/Projects")
}

/// Letters, digits and dashes; starts with a letter.
pub fn valid_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase()) && n.len() <= 40 && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// "My Game!" → "my-game".
pub fn tidy_name(n: &str) -> String {
    let mut s = String::new();
    for c in n.trim().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) { format!("p-{s}") } else { s }
}

pub struct Project {
    pub name: String,
    pub dir: PathBuf,
    pub lang: Lang,
    /// what the program asks to use, from workshop.toml; None: not said,
    /// so it is asked about everything
    pub permissions: Option<String>,
}

impl Project {
    /// The program's name among the apps, for its permissions.
    pub fn app_id(&self) -> String {
        format!("work.{}", tidy_name(&self.name))
    }
}

/// The project a folder belongs to: it or the nearest one above it with a
/// workshop.toml.
pub fn find(start: &Path) -> Option<Project> {
    let mut d = Some(start);
    while let Some(dir) = d {
        let toml = dir.join("workshop.toml");
        if toml.is_file() {
            let text = std::fs::read_to_string(&toml).ok()?;
            let get = |k: &str| {
                text.lines().find_map(|l| {
                    let (a, b) = l.split_once('=')?;
                    (a.trim() == k).then(|| b.trim().trim_matches('"').to_string())
                })
            };
            let name = get("name").unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            let lang = get("language").and_then(|l| Lang::parse(&l))?;
            return Some(Project { name, dir: dir.to_path_buf(), lang, permissions: get("permissions") });
        }
        d = dir.parent();
    }
    None
}

pub fn is_project(dir: &Path) -> bool {
    dir.join("workshop.toml").is_file()
}

fn write(dir: &Path, rel: &str, body: &str) -> std::io::Result<()> {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(p, body)
}

/// Makes a new project with a small working program in it.
pub fn create(name: &str, lang: Lang) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err("A project name is small letters, digits and dashes, starting with a letter.".into());
    }
    let dir = projects_dir().join(name);
    if dir.exists() {
        return Err(format!("There is already a project called {name}."));
    }
    let module = name.replace('-', "_");
    let w = |rel: &str, body: String| write(&dir, rel, &body).map_err(|e| e.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    w(
        "workshop.toml",
        format!(
            "# A Workshop project: `work check | format | test | run | build`.\n\
             name = \"{name}\"\n\
             language = \"{}\"\n\
             # What the program asks to use when it runs: network, camera, sound,\n\
             # files (your personal files). Nothing by default. Chlorum asks you\n\
             # once about each; change the answers in Center → Apps (work.{name}).\n\
             permissions = \"\"\n",
            lang.key()
        ),
    )?;
    w(
        "workshop.nix",
        format!(
            "# The tools of this project. Nothing here is installed on the system:\n\
             # `work` opens them only inside this project's box. Add what you need.\n\
             {{ pkgs ? import <nixpkgs> {{ }} }}:\n\
             pkgs.mkShell {{\n  packages = with pkgs; [ git {} ];\n}}\n",
            lang.packages()
        ),
    )?;
    let ignore = match lang {
        Lang::C => "build/\nrelease/\n",
        Lang::Rust => "target/\n",
        Lang::Python => "__pycache__/\n.venv/\n.ruff_cache/\n.pytest_cache/\n",
        Lang::Web => "node_modules/\ndist/\n",
    };
    w(".gitignore", format!(".workshop/\n{ignore}"))?;
    w("README.md", format!("# {name}\n\nA Workshop project ({}). In its folder:\n\n    work run      start it\n    work test     run the tests\n    work check    warnings and style\n    work format   tidy the code\n    work build    a finished build\n    work enter    a shell with its tools\n", lang.label()))?;
    match lang {
        Lang::C => {
            w(
                "meson.build",
                format!(
                    "project('{name}', 'c', version: '0.1.0', default_options: ['c_std=c17'])\n\n\
                     lib = static_library('{module}', 'src/greet.c', include_directories: 'include')\n\
                     exe = executable('{name}', 'src/main.c', include_directories: 'include', link_with: lib, install: true)\n\
                     test('greet', executable('test-greet', 'tests/test_greet.c', include_directories: 'include', link_with: lib))\n"
                ),
            )?;
            w(".clang-format", "BasedOnStyle: LLVM\nIndentWidth: 4\nColumnLimit: 100\n".into())?;
            w("include/greet.h", "#ifndef GREET_H\n#define GREET_H\n\nconst char *greeting(void);\n\n#endif\n".into())?;
            w("src/greet.c", "#include \"greet.h\"\n\nconst char *greeting(void) { return \"Hello from Chlorum's Workshop\"; }\n".into())?;
            w("src/main.c", "#include <stdio.h>\n\n#include \"greet.h\"\n\nint main(void) {\n    puts(greeting());\n    return 0;\n}\n".into())?;
            w(
                "tests/test_greet.c",
                "#include <string.h>\n\n#include \"greet.h\"\n\nint main(void) { return strstr(greeting(), \"Hello\") ? 0 : 1; }\n".into(),
            )?;
        }
        Lang::Rust => {
            w("Cargo.toml", format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n"))?;
            w(
                "src/main.rs",
                "fn greeting() -> &'static str {\n    \"Hello from Chlorum's Workshop\"\n}\n\nfn main() {\n    println!(\"{}\", greeting());\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn greets() {\n        assert!(super::greeting().starts_with(\"Hello\"));\n    }\n}\n".into(),
            )?;
        }
        Lang::Python => {
            w(
                "pyproject.toml",
                format!(
                    "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.12\"\n\n\
                     [tool.pytest.ini_options]\npythonpath = [\"src\"]\n\n\
                     [tool.ruff]\nline-length = 100\n\n\
                     [tool.pyright]\nextraPaths = [\"src\"]\n"
                ),
            )?;
            w(&format!("src/{module}/__init__.py"), "def greeting() -> str:\n    return \"Hello from Chlorum's Workshop\"\n".into())?;
            w(&format!("src/{module}/__main__.py"), format!("from {module} import greeting\n\nprint(greeting())\n"))?;
            w("tests/test_greeting.py", format!("from {module} import greeting\n\n\ndef test_greeting() -> None:\n    assert greeting().startswith(\"Hello\")\n"))?;
        }
        Lang::Web => {
            w("package.json", format!("{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"type\": \"module\",\n  \"private\": true\n}}\n"))?;
            w(
                "tsconfig.json",
                "{\n  \"compilerOptions\": {\n    \"target\": \"es2023\",\n    \"module\": \"nodenext\",\n    \"lib\": [\"es2023\", \"dom\"],\n    \"strict\": true,\n    \"noEmit\": true,\n    \"allowImportingTsExtensions\": true,\n    \"erasableSyntaxOnly\": true,\n    \"types\": []\n  },\n  \"include\": [\"src\"]\n}\n".into(),
            )?;
            w(".prettierignore", ".workshop/\nnode_modules/\n".into())?;
            w("src/greet.ts", "export function greeting(): string {\n  return \"Hello from Chlorum's Workshop\";\n}\n".into())?;
            w("src/main.ts", "import { greeting } from \"./greet.ts\";\n\nconsole.log(greeting());\n".into())?;
            w(
                "test/greet.test.ts",
                "import { test } from \"node:test\";\nimport assert from \"node:assert/strict\";\nimport { greeting } from \"../src/greet.ts\";\n\ntest(\"greets\", () => {\n  assert.ok(greeting().startsWith(\"Hello\"));\n});\n".into(),
            )?;
        }
    }
    // its own history from the first minute
    let _ = std::process::Command::new("git").args(["init", "-q"]).current_dir(&dir).status();
    Ok(dir)
}

/// The tools' box: a bubblewrap sandbox where the home folder is empty
/// except for the project, caches live in the project, and the
/// compositor's control socket is out of reach.
fn boxed(p: &Project, inner: &[String]) -> std::process::Command {
    let home = home();
    let mut c = std::process::Command::new("bwrap");
    c.args(["--dev-bind", "/", "/", "--tmpfs"]).arg(&home);
    if let Some(n) = std::env::var_os("NIRI_SOCKET").filter(|n| Path::new(n).exists()) {
        c.args(["--ro-bind", "/dev/null"]).arg(n);
    }
    c.args(["--unsetenv", "NIRI_SOCKET"]);
    inside(&mut c, p, inner);
    c
}

/// The program's box: what it may use is the owner's answer (see
/// sandbox.rs), the same as for any app; asked the first time.
fn program_box(p: &Project, inner: &[String]) -> std::process::Command {
    use crate::sandbox::{self, Perm};
    let app = p.app_id();
    let wants = sandbox::needs(&app, p.permissions.as_deref());
    let off = sandbox::resolve(&app, &p.name, "applications-development", &wants);
    let on: Vec<&str> = Perm::ALL.iter().filter(|x| !off.contains(x)).map(|x| x.key()).collect();
    eprintln!(
        "work: {} may use {} (asked for in workshop.toml, answered in Center → Apps → {app})",
        p.name,
        if on.is_empty() { "nothing".to_string() } else { on.join(", ") }
    );
    let home = home();
    sandbox::ensure_protected(&home);
    if off.contains(&Perm::Files) {
        let _ = std::fs::create_dir_all(sandbox::drop_box(&app));
        let _ = std::fs::create_dir_all(sandbox::private_home(&app));
    }
    let (devices, outside) = sandbox::prepare(&app);
    let mut c = std::process::Command::new("bwrap");
    c.args(sandbox::bwrap_args(&app, &off, &sandbox::runtime(), &home, &devices, &outside, &[]));
    if off.contains(&Perm::Files) {
        // nothing of yours, only the project
        c.arg("--tmpfs").arg(&home);
    }
    c.args(["--setenv", "CHLORUM_APP", &app]);
    if off.contains(&Perm::Files) {
        c.args(["--setenv", "CHLORUM_NO_FILES", "1"]);
    }
    inside(&mut c, p, inner);
    c
}

/// What both boxes share: the project, its caches, its own home.
fn inside(c: &mut std::process::Command, p: &Project, inner: &[String]) {
    let home = home();
    let ws = p.dir.join(".workshop");
    for sub in ["home", "cache", "config", "data", "cargo", "npm", "uv"] {
        let _ = std::fs::create_dir_all(ws.join(sub));
    }
    let s = |p: &Path| p.to_string_lossy().to_string();
    c.args(["--bind"]).arg(s(&p.dir)).arg(s(&p.dir));
    // your name and e-mail for commits, read-only
    for f in [".gitconfig", ".config/git"] {
        let src = home.join(f);
        if src.exists() {
            c.arg("--ro-bind").arg(s(&src)).arg(s(&ws.join("home").join(f)));
        }
    }
    c.args(["--die-with-parent", "--chdir"]).arg(s(&p.dir));
    let env = [
        ("HOME", ws.join("home")),
        ("XDG_CACHE_HOME", ws.join("cache")),
        ("XDG_CONFIG_HOME", ws.join("config")),
        ("XDG_DATA_HOME", ws.join("data")),
        ("CARGO_HOME", ws.join("cargo")),
        ("npm_config_cache", ws.join("npm")),
        ("UV_CACHE_DIR", ws.join("uv")),
    ];
    for (k, v) in &env {
        c.arg("--setenv").arg(k).arg(s(v));
    }
    c.args(["--setenv", "WORKSHOP", &p.name, "--setenv", "PYTHONDONTWRITEBYTECODE", "1"]);
    c.arg("--").args(inner);
}

/// nix-shell with only the project's tools (plus what the box passes on).
fn in_shell(run: Option<String>) -> Vec<String> {
    let mut v: Vec<String> = ["nix-shell", "--pure", "workshop.nix"].iter().map(|s| s.to_string()).collect();
    for k in [
        "HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR", "CARGO_HOME", "npm_config_cache", "UV_CACHE_DIR",
        "WORKSHOP", "PYTHONDONTWRITEBYTECODE", "WAYLAND_DISPLAY", "DISPLAY", "TERM", "LANG", "PS1",
        "CHLORUM_APP", "CHLORUM_NO_FILES", "DBUS_SESSION_BUS_ADDRESS",
    ] {
        v.push("--keep".into());
        v.push(k.into());
    }
    match run {
        Some(cmd) => {
            v.push("--run".into());
            v.push(cmd);
        }
        None => {
            v.push("--command".into());
            v.push("exec bash --norc".into());
        }
    }
    v
}

fn here() -> Result<Project, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    find(&cwd).ok_or_else(|| "Not in a project. Go into one (Library → Projects) or make one: work new <name> <c|rust|python|web>".into())
}

fn run_in_box(p: &Project, run: Option<String>) -> Result<(), String> {
    if run.is_none() {
        // SAFETY: single-threaded; only our own environment
        unsafe { std::env::set_var("PS1", format!("\\[\\e[1;33m\\][{}]\\[\\e[0m\\] \\w \\$ ", p.name)) };
    }
    finish(boxed(p, &in_shell(run)).status())
}

fn finish(status: std::io::Result<std::process::ExitStatus>) -> Result<(), String> {
    let status = status.map_err(|e| format!("could not open the box: {e}"))?;
    if status.success() { Ok(()) } else { std::process::exit(status.code().unwrap_or(1)) }
}

/// `work run` / `work test`: get ready in the tools' box, then start the
/// program in its own box, with its permissions.
fn run_program(p: &Project, what: &str) -> Result<(), String> {
    if let Some(prep) = p.lang.prepare(what) {
        let ok = boxed(p, &in_shell(Some(prep))).status().map_err(|e| format!("could not open the box: {e}"))?;
        if !ok.success() {
            std::process::exit(ok.code().unwrap_or(1));
        }
    }
    let line = p.lang.command(what, &p.name).ok_or("unknown command")?;
    finish(program_box(p, &in_shell(Some(line))).status())
}

const HELP: &str = "work — programming without cluttering the system

  work new <name> <c|rust|python|web>   a new project in Library → Projects
  work check | format | test | run | build
  work enter                             a shell with the project's tools
  work list                              the projects
  work clean                             remove caches and build output

Run them inside a project's folder.";

pub fn cli(args: &[String]) -> Result<(), String> {
    let what = args.first().map(String::as_str).unwrap_or("help");
    match what {
        "help" | "-h" | "--help" => {
            println!("{HELP}");
            Ok(())
        }
        "new" => {
            let (Some(name), Some(lang)) = (args.get(1), args.get(2)) else { return Err("usage: work new <name> <c|rust|python|web>".into()) };
            let lang = Lang::parse(lang).ok_or("Languages: c, rust, python, web")?;
            let name = tidy_name(name);
            let dir = create(&name, lang)?;
            println!("Made {} ({}). Next:\n  cd {}\n  work run", name, lang.label(), dir.display());
            Ok(())
        }
        "list" => {
            let mut v: Vec<_> = std::fs::read_dir(projects_dir()).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            v.sort();
            for d in v.iter().filter(|d| is_project(d)) {
                if let Some(p) = find(d) {
                    println!("{:<24} {}", p.name, p.lang.label());
                }
            }
            Ok(())
        }
        "enter" => run_in_box(&here()?, None),
        "clean" => {
            let p = here()?;
            let mut gone = vec![".workshop"];
            gone.extend(match p.lang {
                Lang::C => vec!["build", "release"],
                Lang::Rust => vec!["target"],
                Lang::Python => vec![".venv", ".ruff_cache", ".pytest_cache"],
                Lang::Web => vec!["node_modules", "dist"],
            });
            for g in gone {
                let _ = std::fs::remove_dir_all(p.dir.join(g));
            }
            println!("Clean: only your code is left in {}.", p.name);
            Ok(())
        }
        cmd if PROGRAM.contains(&cmd) => run_program(&here()?, cmd),
        cmd if COMMANDS.contains(&cmd) => {
            let p = here()?;
            let line = p.lang.command(cmd, &p.name).ok_or("unknown command")?;
            run_in_box(&p, Some(line))
        }
        other => Err(format!("unknown: {other}\n\n{HELP}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(tidy_name("My Game!"), "my-game");
        assert_eq!(tidy_name("  2048 clone "), "p-2048-clone");
        assert!(valid_name("hello-world"));
        assert!(!valid_name("Hello"));
        assert!(!valid_name("-x"));
    }

    #[test]
    fn every_language_has_every_command() {
        for l in Lang::ALL {
            for c in COMMANDS {
                assert!(l.command(c, "demo-app").is_some(), "{l:?} {c}");
            }
        }
        assert_eq!(Lang::Python.command("run", "demo-app").unwrap(), "PYTHONPATH=src python -m demo_app");
    }
}
