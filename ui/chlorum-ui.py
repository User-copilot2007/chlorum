#!/usr/bin/env python3
"""Chlorum interface backends. Standard library only.

  chlorum-ui greeter   first run and login, inside cage under greetd (user "greeter")
  chlorum-ui setupd    privileged helper for the first run (root, until setup is done)
  chlorum-ui shell     main menu inside the desktop session (the logged-in person)

The pages (index.html, app.js) are served on 127.0.0.1 and shown in a Firefox
kiosk window. Every start makes a random token; requests without it are refused.
"""
import json
import os
import re
import secrets
import shutil
import socket
import socketserver
import struct
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

UI_DIR = os.environ.get("CHLORUM_UI_DIR", os.path.dirname(os.path.abspath(__file__)))
STATE = os.environ.get("CHLORUM_STATE_DIR", "/var/lib/chlorum")
SETUP_DONE = f"{STATE}/setup-done"
RUN_DIR = os.environ.get("CHLORUM_RUN_DIR", "/run/chlorum")
SETUP_SOCK = f"{RUN_DIR}/setup.sock"
SESSION_CMD = os.environ.get("CHLORUM_SESSION_CMD", "chlorum-session")

LAYOUTS = [  # (xkb layout, console keymap, name)
    ("us", "us", "English (US)"),
    ("gb", "uk", "English (UK)"),
    ("fr", "fr", "French (AZERTY)"),
    ("be", "be-latin1", "Belgian (AZERTY)"),
    ("de", "de", "German (QWERTZ)"),
    ("ch", "de_CH-latin1", "Swiss"),
    ("es", "es", "Spanish"),
    ("it", "it", "Italian"),
    ("pl", "pl", "Polish"),
    ("ua", "ua", "Ukrainian"),
]

ADJECTIVES = """brave calm clever cosmic crisp curious daring eager fluffy frosty
gentle giant golden happy hasty humble jolly keen lazy lively lucky mellow mighty
misty nimble noble odd plucky polite proud quick quiet rapid rusty shiny silent
sleepy sly snowy speedy spicy steady sunny swift tidy tiny vivid wild witty zesty""".split()
ANIMALS = """badger beaver bison camel cobra cougar coyote crane dingo falcon ferret
gecko gopher heron hippo husky ibis iguana jackal koala lemur llama lynx magpie
marmot mole moose newt ocelot otter owl panda parrot pelican puffin quokka rabbit
raven salmon seal sloth tapir toucan turtle walrus weasel wombat yak zebra lobster""".split()

NAME_RE = re.compile(r"^[a-z][a-z0-9-]{1,30}$")


def log(*a):
    print("chlorum-ui:", *a, file=sys.stderr, flush=True)


def new_name():
    return f"{secrets.choice(ADJECTIVES)}-{secrets.choice(ANIMALS)}"


def run(cmd, env=None, timeout=120):
    """Run a command, return (ok, combined output)."""
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout,
                           env={**os.environ, **(env or {})})
        return p.returncode == 0, (p.stdout + p.stderr).strip()
    except Exception as e:  # missing tool, timeout
        return False, str(e)


def read(path, default=""):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return default


def list_profiles():
    ok, out = run(["homectl", "list", "--no-legend"], timeout=20)
    if not ok:
        return []
    return [line.split()[0] for line in out.splitlines() if line.strip()]


# ---------------------------------------------------------------- web pages

class Web(BaseHTTPRequestHandler):
    """Serves the pages and dispatches /api/<name> to self.server.api."""
    TYPES = {".html": "text/html", ".js": "text/javascript", ".css": "text/css",
             ".svg": "image/svg+xml", ".woff2": "font/woff2", ".png": "image/png"}

    def log_message(self, *a):
        pass

    def reply(self, code, data=None, body=None, ctype="application/json"):
        if body is None:
            body = json.dumps(data or {}).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def handle_any(self, method):
        url = urlparse(self.path)
        if url.path.startswith("/api/"):
            if parse_qs(url.query).get("t", [""])[0] != self.server.token:
                return self.reply(403, {"error": "Not allowed."})
            body = None
            if method == "POST":
                n = int(self.headers.get("Content-Length") or 0)
                try:
                    body = json.loads(self.rfile.read(n) or b"{}")
                except ValueError:
                    return self.reply(400, {"error": "Bad request."})
            try:
                res = self.server.api(method, url.path[5:], body)
            except Exception as e:
                log("api error:", repr(e))
                res = {"error": "Something went wrong. Please try again."}
            return self.reply(200, res)
        if method != "GET":
            return self.reply(405, {"error": "Not allowed."})
        name = os.path.basename(url.path) or "index.html"
        sub = "fonts" if url.path.startswith("/fonts/") else ""
        path = os.path.join(UI_DIR, sub, name)
        if not os.path.isfile(path):
            return self.reply(404, {"error": "Not found."})
        with open(path, "rb") as f:
            self.reply(200, body=f.read(),
                       ctype=self.TYPES.get(os.path.splitext(name)[1], "application/octet-stream"))

    def do_GET(self):
        self.handle_any("GET")

    def do_POST(self):
        self.handle_any("POST")


def serve(api):
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Web)
    srv.token = secrets.token_urlsafe(24)
    srv.api = api
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def kiosk(url, name, profile_dir):
    """A Firefox window with no browser interface, showing one Chlorum page."""
    os.makedirs(profile_dir, exist_ok=True)
    with open(os.path.join(profile_dir, "user.js"), "w") as f:
        f.write('user_pref("browser.shell.checkDefaultBrowser", false);\n'
                'user_pref("browser.sessionstore.resume_from_crash", false);\n'
                'user_pref("toolkit.legacyUserProfileCustomizations.stylesheets", true);\n'
                'user_pref("browser.startup.homepage_override.mstone", "ignore");\n'
                'user_pref("datareporting.policy.dataSubmissionEnabled", false);\n'
                'user_pref("browser.preonboarding.enabled", false);\n'
                'user_pref("toolkit.startup.max_resumed_crashes", -1);\n')
    return subprocess.Popen(["firefox", "--kiosk", "--no-remote", "--new-instance",
                             "--name", name, "--profile", profile_dir, url])


# ---------------------------------------------------------------- greetd

class Greetd:
    """greetd IPC: native-endian u32 length + JSON."""

    def __init__(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(os.environ["GREETD_SOCK"])

    def call(self, msg):
        data = json.dumps(msg).encode()
        self.sock.sendall(struct.pack("=I", len(data)) + data)
        n = struct.unpack("=I", self._recv(4))[0]
        return json.loads(self._recv(n))

    def _recv(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionError("greetd closed the connection")
            buf += chunk
        return buf


GREETD_LOCK = threading.Lock()


def greetd_login(user, password):
    """Returns None on success (session started), or an error text."""
    with GREETD_LOCK:  # one login attempt at a time
        g = Greetd()
        r = g.call({"type": "create_session", "username": user})
        answered, message = False, ""
        while r.get("type") == "auth_message":
            kind = r.get("auth_message_type")
            if kind in ("secret", "visible"):
                if answered:
                    # PAM asks again: the password was wrong. Stop here instead
                    # of spending more attempts on the same wrong password.
                    g.call({"type": "cancel_session"})
                    return message or "Wrong profile name or password."
                answered = True
                r = g.call({"type": "post_auth_message_response", "response": password})
            else:  # information or error text from PAM: keep it, acknowledge
                if kind == "error" and r.get("auth_message"):
                    message = r["auth_message"].strip()
                r = g.call({"type": "post_auth_message_response", "response": None})
        if r.get("type") != "success":
            g.call({"type": "cancel_session"})
            if message:
                return message
            if r.get("error_type") == "auth_error":
                return "Wrong profile name or password."
            return r.get("description") or "Could not log in."
        r = g.call({"type": "start_session", "cmd": [SESSION_CMD], "env": []})
        if r.get("type") != "success":
            g.call({"type": "cancel_session"})
            return r.get("description") or "Could not start the desktop."
        return None


# ---------------------------------------------------------------- setup helper (root)

def setup_call(msg):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(300)
    try:
        s.connect(SETUP_SOCK)
    except OSError:
        return {"error": "The setup helper is not running yet. Please wait a moment and try again."}
    s.sendall(json.dumps(msg).encode() + b"\n")
    f = s.makefile("r")
    line = f.readline()
    s.close()
    return json.loads(line) if line else {"error": "The setup helper did not answer."}


class Setupd:
    def __init__(self):
        self.proposal = None
        self.lock = threading.Lock()
        os.makedirs(STATE, exist_ok=True)
        if not read(f"{STATE}/hostname"):
            with open(f"{STATE}/hostname", "w") as f:
                f.write(new_name() + "\n")
        try:
            with open(os.environ.get("CHLORUM_HOSTNAME_FILE", "/proc/sys/kernel/hostname"), "w") as f:
                f.write(read(f"{STATE}/hostname"))
        except OSError as e:
            log("hostname:", e)

    def handle(self, req):
        with self.lock:
            op = req.get("op")
            fn = getattr(self, "op_" + str(op), None)
            if fn is None or os.path.exists(SETUP_DONE):
                return {"error": "Not allowed."}
            return fn(req)

    def op_layouts(self, req):
        return {"layouts": [{"id": x, "name": n} for x, _, n in LAYOUTS],
                "current": read(f"{STATE}/xkb-layout")}

    def op_set_layout(self, req):
        for xkb, console, _ in LAYOUTS:
            if req.get("id") == xkb:
                with open(f"{STATE}/xkb-layout", "w") as f:
                    f.write(xkb + "\n")
                with open(f"{STATE}/keymap", "w") as f:
                    f.write(console + "\n")
                run(["loadkeys", "-q", console], timeout=20)
                return {"ok": True}
        return {"error": "Unknown layout."}

    def op_network(self, req):
        ok, out = run(["nmcli", "-t", "-f", "CONNECTIVITY", "general"], timeout=20)
        if ok and out.strip() == "full":
            return {"online": True}
        ok, out = run(["nmcli", "-t", "-f", "SSID,SIGNAL,SECURITY", "device", "wifi",
                       "list", "--rescan", "yes"], timeout=40)
        nets, seen = [], set()
        for line in out.splitlines() if ok else []:
            parts = re.split(r"(?<!\\):", line)
            if len(parts) < 3 or not parts[0] or parts[0] in seen:
                continue
            ssid = parts[0].replace("\\:", ":")
            seen.add(parts[0])
            nets.append({"ssid": ssid, "signal": int(parts[1] or 0),
                         "secure": parts[2] not in ("", "--")})
        nets.sort(key=lambda n: -n["signal"])
        return {"online": False, "wifi": nets[:12]}

    def op_wifi(self, req):
        ssid, pw = str(req.get("ssid") or ""), str(req.get("password") or "")
        if not ssid:
            return {"error": "Choose a network."}
        cmd = ["nmcli", "device", "wifi", "connect", ssid]
        if pw:
            cmd += ["password", pw]
        ok, out = run(cmd, timeout=60)
        if not ok:
            return {"error": "Could not connect. Check the password and try again."}
        return {"ok": True}

    def op_proposal(self, req):
        if self.proposal is None:
            ok, pw = run(["xkcdpass", "-w", "eff-long", "-n", "5", "-d", "-"], timeout=20)
            if not ok or not pw:
                return {"error": "Could not make a password."}
            name = new_name()
            while name == read(f"{STATE}/hostname"):
                name = new_name()
            self.proposal = {"name": name, "password": pw.splitlines()[0]}
        return self.proposal

    def op_create(self, req):
        name, pw = str(req.get("name") or ""), str(req.get("password") or "")
        if not NAME_RE.match(name):
            return {"error": "Use 2 to 31 lowercase letters, digits or -, starting with a letter."}
        if len(pw) < 12:
            return {"error": "The password must be at least 12 characters."}
        # A profile left by an earlier attempt was never shown to anyone.
        for old in list_profiles():
            run(["homectl", "remove", old], timeout=120)
        ok, out = run(["homectl", "create", name, f"--real-name={name}",
                       "--member-of=chlorum-owner", "--storage=luks", "--fs-type=btrfs",
                       "--enforce-password-policy=no", "--luks-discard=yes"],
                      env={"NEWPASSWORD": pw}, timeout=600)
        if not ok:
            log("homectl create failed:", out)
            return {"error": out.splitlines()[-1] if out else "Could not create the profile."}
        return {"ok": True}

    def op_finish(self, req):
        if not list_profiles():
            return {"error": "No profile was created yet."}
        open(SETUP_DONE, "w").close()
        threading.Timer(1.0, lambda: os._exit(0)).start()
        return {"ok": True}


def cmd_setupd():
    if os.path.exists(SETUP_DONE):
        return
    helper = Setupd()
    os.makedirs(RUN_DIR, exist_ok=True)
    try:
        os.unlink(SETUP_SOCK)
    except OSError:
        pass
    gid = int(subprocess.run(["getent", "group", "greeter"], capture_output=True,
                             text=True).stdout.split(":")[2])
    os.chown(RUN_DIR, 0, gid)
    os.chmod(RUN_DIR, 0o750)

    class H(socketserver.StreamRequestHandler):
        def handle(self):
            line = self.rfile.readline()
            try:
                res = helper.handle(json.loads(line))
            except Exception as e:
                log("setupd error:", repr(e))
                res = {"error": "Something went wrong. Please try again."}
            self.wfile.write(json.dumps(res).encode() + b"\n")

    class S(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
        daemon_threads = True

    srv = S(SETUP_SOCK, H)
    os.chown(SETUP_SOCK, 0, gid)
    os.chmod(SETUP_SOCK, 0o660)
    log("setup helper ready")
    srv.serve_forever()


# ---------------------------------------------------------------- greeter

def cmd_greeter():
    restart = threading.Event()

    def api(method, name, body):
        if name == "state":
            return {"setup": not os.path.exists(SETUP_DONE),
                    "layout_done": bool(read(f"{STATE}/xkb-layout")),
                    "hostname": read(f"{STATE}/hostname"),
                    "profiles": list_profiles()}
        if name == "login" and method == "POST":
            err = greetd_login(str(body.get("user", "")), str(body.get("password", "")))
            if err:
                return {"error": err}
            threading.Timer(0.5, lambda: os._exit(0)).start()
            return {"ok": True}
        if name == "power" and method == "POST":
            action = body.get("action")
            if action in ("poweroff", "reboot"):
                ok, out = run(["systemctl", action])
                return {"ok": True} if ok else {"error": "Not possible right now."}
            return {"error": "Unknown action."}
        if name.startswith("setup/"):
            op = name[6:]
            msg = dict(body or {})
            msg["op"] = {"layout": "set_layout"}.get(op, op) if method == "POST" else op
            res = setup_call(msg)
            if op == "layout" and method == "POST" and res.get("ok"):
                # The new keyboard layout applies when this window restarts.
                restart.set()
            return res
        return {"error": "Unknown request."}

    srv = serve(api)
    url = f"http://127.0.0.1:{srv.server_address[1]}/?mode=greeter&t={srv.token}"
    home = tempfile.mkdtemp(prefix="chlorum-greeter-")
    os.environ["HOME"] = home
    profile = os.path.join(home, "profile")
    browser = kiosk(url, "chlorum-greeter", profile)
    while True:
        if restart.wait(0.5):
            time.sleep(0.5)
            break
        if browser.poll() is not None:
            # The window died (graphics not ready, crash): show it again rather
            # than ending the greeter, which could leave a black screen.
            log("kiosk window exited, starting it again")
            time.sleep(2)
            browser = kiosk(url, "chlorum-greeter", profile)
    browser.terminate()
    shutil.rmtree(home, ignore_errors=True)
    # Leaving ends this greeter; greetd starts it again with the new layout.
    os._exit(0)


# ---------------------------------------------------------------- desktop menu

def cmd_shell():
    update = {"started": False}

    def sway(*args):
        run(["swaymsg", *args], timeout=10)

    def api(method, name, body):
        if name == "info":
            return {"user": os.environ.get("USER", ""), "hostname": read(f"{STATE}/hostname")}
        if name == "launch" and method == "POST":
            app = body.get("app")
            cmd = {"firefox": ["firefox"], "terminal": ["foot"]}.get(app)
            if not cmd:
                return {"error": "Unknown app."}
            sway("workspace", "number", "1")
            subprocess.Popen(cmd, start_new_session=True,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return {"ok": True}
        if name == "power" and method == "POST":
            action = body.get("action")
            if action == "logout":
                sway("exit")
                return {"ok": True}
            if action in ("poweroff", "reboot"):
                ok, _ = run(["systemctl", action])
                return {"ok": True} if ok else {"error": "Not possible right now."}
            return {"error": "Unknown action."}
        if name == "update":
            if method == "POST":
                ok, out = run(["systemctl", "start", "--no-block", "nixos-upgrade.service"])
                if not ok:
                    return {"error": "Could not start the update."}
                update["started"] = True
                return {"ok": True}
            _, active = run(["systemctl", "show", "-P", "ActiveState", "nixos-upgrade.service"])
            _, result = run(["systemctl", "show", "-P", "Result", "nixos-upgrade.service"])
            if active in ("activating", "active", "reloading"):
                return {"state": "running"}
            if not update["started"]:
                return {"state": "idle"}
            return {"state": "done" if result == "success" else "failed"}
        return {"error": "Unknown request."}

    srv = serve(api)
    url = f"http://127.0.0.1:{srv.server_address[1]}/?mode=menu&t={srv.token}"
    state = os.path.join(os.environ.get("XDG_STATE_HOME", os.path.expanduser("~/.local/state")),
                         "chlorum", "menu-profile")
    # Keep the menu alive for the session, and end with it: a process left
    # behind would keep the encrypted profile open after logging out.
    while True:
        browser = kiosk(url, "chlorum-menu", state)
        browser.wait()
        time.sleep(2)
        alive, _ = run(["swaymsg", "-t", "get_version"], timeout=5)
        if not alive:
            os._exit(0)


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    {"greeter": cmd_greeter, "setupd": cmd_setupd, "shell": cmd_shell}.get(
        cmd, lambda: sys.exit("usage: chlorum-ui greeter|setupd|shell"))()


if __name__ == "__main__":
    main()
