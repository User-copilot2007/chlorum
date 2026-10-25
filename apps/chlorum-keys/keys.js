// Chlorum Keys: every web page works without a mouse.
//
//   ← ↑ ↓ →   move to the nearest button or link in that direction
//              (when there is none on screen, the page scrolls as usual)
//   Enter      press it
//   F          put a letter on every button and link; type it to press
//   Esc        leave letter mode
//
// Pages that use these keys themselves (games, video players, editors)
// keep them: Chlorum Keys only acts when the page did not.

(() => {
  if (window.__chlorumKeys) return;
  window.__chlorumKeys = true;

  const LETTERS = "fjdkslaghrueiwoqtypvnmcxzb";
  const CLICKABLE = [
    "a[href]", "button", "input:not([type=hidden])", "select", "textarea", "summary",
    "label[for]", "[role=button]", "[role=link]", "[role=tab]", "[role=menuitem]",
    "[role=checkbox]", "[role=switch]", "[role=option]", "[role=radio]", "[onclick]",
    "[tabindex]:not([tabindex='-1'])", "[contenteditable='']", "[contenteditable='true']",
  ].join(",");
  const TEXT_TYPES = new Set(["", "text", "search", "email", "url", "tel", "password", "number",
    "date", "datetime-local", "month", "time", "week"]);

  function typing(el) {
    if (!el) return false;
    if (el.isContentEditable) return true;
    if (el.tagName === "TEXTAREA" || el.tagName === "SELECT") return true;
    if (el.tagName === "INPUT") return TEXT_TYPES.has((el.getAttribute("type") || "").toLowerCase());
    return false;
  }

  function onScreen(el) {
    const r = el.getBoundingClientRect();
    if (r.width < 3 || r.height < 3) return null;
    if (r.bottom <= 0 || r.right <= 0 || r.top >= innerHeight || r.left >= innerWidth) return null;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none" || Number(cs.opacity) === 0) return null;
    if (el.disabled) return null;
    // something else may cover it: look at a few of its points
    const pts = [[0.5, 0.5], [0.2, 0.5], [0.8, 0.5], [0.5, 0.2], [0.5, 0.8]];
    for (const [fx, fy] of pts) {
      const x = Math.min(Math.max(r.left + r.width * fx, 0), innerWidth - 1);
      const y = Math.min(Math.max(r.top + r.height * fy, 0), innerHeight - 1);
      const top = document.elementFromPoint(x, y);
      if (top && (top === el || el.contains(top) || top.contains(el) ||
          (el.tagName === "INPUT" && top.tagName === "LABEL" && top.control === el))) return r;
    }
    return null;
  }

  function targets() {
    const out = [];
    const seen = new Set();
    for (const el of document.querySelectorAll(CLICKABLE)) {
      // a link wrapping a button counts once
      if (seen.has(el.parentElement) && el.closest("a[href],button") !== el) continue;
      const r = onScreen(el);
      if (r) { out.push({ el, r }); seen.add(el); }
    }
    return out;
  }

  function press(el) {
    if (typing(el)) { el.focus(); return; }
    el.focus({ preventScroll: true });
    el.click();
  }

  // ------------------------------------------------------------ letters

  let hint = null; // { host, items: [{code, el, tag}], typed }

  function codes(n) {
    if (n <= LETTERS.length) return LETTERS.slice(0, n).split("");
    const out = [];
    for (const a of LETTERS) for (const b of LETTERS) { out.push(a + b); if (out.length === n) return out; }
    return out;
  }

  function showHints() {
    const list = targets();
    if (!list.length) return;
    const host = document.createElement("chlorum-keys");
    host.style.cssText = "all:initial;position:fixed;inset:0;z-index:2147483647;pointer-events:none;";
    const root = host.attachShadow({ mode: "closed" });
    const style = document.createElement("style");
    style.textContent = `
      .t{position:fixed;font:600 13px/1 "IBM Plex Mono",ui-monospace,monospace;letter-spacing:.04em;
         background:#FFD60A;color:#000;padding:3px 5px;box-shadow:0 0 0 2px #000;text-transform:uppercase}
      .t b{color:rgba(0,0,0,.35);font-weight:600}
      .bar{position:fixed;left:50%;bottom:24px;transform:translateX(-50%);background:#000;color:#f5f5f5;
         border:3px solid #FFD60A;padding:10px 18px;font:15px/1.2 "IBM Plex Mono",ui-monospace,monospace}
      .bar span{color:#FFD60A}`;
    root.appendChild(style);
    const cs = codes(list.length);
    const items = list.map(({ el, r }, i) => {
      const tag = document.createElement("div");
      tag.className = "t";
      tag.textContent = cs[i];
      tag.style.left = Math.max(0, r.left - 4) + "px";
      tag.style.top = Math.max(0, r.top - 4) + "px";
      root.appendChild(tag);
      return { code: cs[i], el, tag };
    });
    const bar = document.createElement("div");
    bar.className = "bar";
    bar.innerHTML = "<span>JUMP</span>&nbsp;&nbsp;Type a letter to press that button · Esc cancels";
    root.appendChild(bar);
    document.documentElement.appendChild(host);
    hint = { host, items, typed: "" };
  }

  function hideHints() {
    if (hint) hint.host.remove();
    hint = null;
  }

  function hintKey(e) {
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.key === "Escape") return hideHints();
    if (e.key === "Backspace") hint.typed = hint.typed.slice(0, -1);
    else if (e.key.length === 1 && /[a-z]/i.test(e.key)) hint.typed += e.key.toLowerCase();
    else return;
    const left = hint.items.filter((it) => it.code.startsWith(hint.typed));
    if (left.length === 0) { hint.typed = hint.typed.slice(0, -1); return; }
    for (const it of hint.items) {
      const on = it.code.startsWith(hint.typed);
      it.tag.style.display = on ? "" : "none";
      if (on) it.tag.innerHTML = `<b>${hint.typed}</b>${it.code.slice(hint.typed.length)}`;
    }
    const exact = left.find((it) => it.code === hint.typed);
    if (exact && left.length === 1) {
      hideHints();
      press(exact.el);
    }
  }

  // ------------------------------------------------------------ arrows

  function nearest(dir) {
    const cur = document.activeElement;
    let o = cur && cur !== document.body && cur.matches && cur.matches(CLICKABLE) ? onScreen(cur) : null;
    if (!o) {
      // nothing chosen yet: start from the edge we move away from
      const w = innerWidth, h = innerHeight;
      o = { left: w / 2, right: w / 2, top: h / 2, bottom: h / 2, width: 0, height: 0 };
      if (dir === "down") { o.top = o.bottom = 0; o.left = 0; o.right = w; }
      if (dir === "up") { o.top = o.bottom = h; o.left = 0; o.right = w; }
      if (dir === "right") { o.left = o.right = 0; o.top = 0; o.bottom = h; }
      if (dir === "left") { o.left = o.right = w; o.top = 0; o.bottom = h; }
    }
    let best = null, bestScore = Infinity;
    for (const { el, r } of targets()) {
      if (el === cur) continue;
      let along, across;
      if (dir === "right") { along = r.left - o.right; across = overlap(o.top, o.bottom, r.top, r.bottom); }
      else if (dir === "left") { along = o.left - r.right; across = overlap(o.top, o.bottom, r.top, r.bottom); }
      else if (dir === "down") { along = r.top - o.bottom; across = overlap(o.left, o.right, r.left, r.right); }
      else { along = o.top - r.bottom; across = overlap(o.left, o.right, r.left, r.right); }
      if (along < -2) continue; // not in that direction
      const score = Math.max(along, 0) + across * 2;
      if (score < bestScore) { bestScore = score; best = el; }
    }
    return best;
  }

  // distance between two ranges on the other axis (0 when they line up)
  function overlap(a1, a2, b1, b2) {
    if (b2 < a1) return a1 - b2;
    if (b1 > a2) return b1 - a2;
    return 0;
  }

  // A one-line text field lets the arrows out: up and down always, left
  // and right when the cursor is already at that end of the text.
  function canLeave(el, dir) {
    if (el.tagName !== "INPUT") return false;
    const type = (el.getAttribute("type") || "").toLowerCase();
    if (type === "number" || type.startsWith("date") || type === "time" || type === "month" || type === "week") return false;
    if (dir === "up" || dir === "down") return true;
    let start, end;
    try { start = el.selectionStart; end = el.selectionEnd; } catch (_) { return true; }
    if (start === null) return true;
    if (start !== end) return false;
    return dir === "left" ? start === 0 : start === el.value.length;
  }

  // ------------------------------------------------------------ keys

  // Letter mode takes every key first.
  window.addEventListener("keydown", (e) => { if (hint) hintKey(e); }, true);

  // Everything else only when the page itself did nothing with the key.
  window.addEventListener("keydown", (e) => {
    if (hint || e.defaultPrevented || e.ctrlKey || e.altKey || e.metaKey) return;
    const el = document.activeElement;
    const dir = { ArrowRight: "right", ArrowLeft: "left", ArrowDown: "down", ArrowUp: "up" }[e.key];
    if (typing(el) && !(dir && canLeave(el, dir))) return;
    if ((e.key === "f" || e.key === "F") && !e.shiftKey && window === window.top) {
      e.preventDefault();
      showHints();
      return;
    }
    if (dir && !e.shiftKey) {
      const next = nearest(dir);
      if (next) {
        e.preventDefault();
        next.focus({ preventScroll: true });
        next.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
      }
      return;
    }
    // Enter presses things that only look like buttons
    if (e.key === "Enter" && el && el.matches && el.matches("[role=button],[role=tab],[role=menuitem],[onclick],[tabindex]")
        && !el.matches("a,button,input,select,textarea,summary")) {
      e.preventDefault();
      el.click();
    }
  }, false);

  window.addEventListener("scroll", hideHints, { passive: true });
  window.addEventListener("resize", hideHints, { passive: true });
})();
