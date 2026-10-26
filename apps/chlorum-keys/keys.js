// Chlorum Keys: every web page works without a mouse.
//
//   ← ↑ ↓ →   move to the nearest button or link in that direction
//              (when there is none on screen, the page scrolls as usual)
//   Enter      press it
//   F          put a letter on every button and link; type it to press
//   Alt .      the same, from anywhere, even while typing in a text box
//   Esc        leave letter mode, or leave the text box the page put you in
//   ?          show these keys
//
// Each can be turned off on the add-on's settings page.
//
// F, Alt . , ? and Esc come first, before the page sees them (YouTube uses F
// for full screen, chat sites put you in their text box). The arrows and
// Enter stay with pages that use them (video players, games, editors).

(() => {
  if (window.__chlorumKeys) return;
  window.__chlorumKeys = true;

  const LETTERS = "fjdkslaghrueiwoqtypvnmcxzb";
  // the owner's choices (settings page); these are the defaults
  const opt = { arrows: true, letters: true, letterKey: "f", escape: true, help: true };
  try {
    browser.storage.local.get(opt).then((v) => Object.assign(opt, v), () => {});
    browser.storage.onChanged.addListener((ch) => { for (const k in ch) opt[k] = ch[k].newValue; });
  } catch (_) { /* outside the add-on (tests): defaults */ }
  const CLICKABLE = [
    "a[href]", "button", "input:not([type=hidden])", "select", "textarea", "summary",
    "label[for]", "[role=button]", "[role=link]", "[role=tab]", "[role=menuitem]",
    "[role=checkbox]", "[role=switch]", "[role=option]", "[role=radio]", "[onclick]",
    "[tabindex]:not([tabindex='-1'])", "[contenteditable='']", "[contenteditable='true']",
  ].join(",");
  const TEXT_TYPES = new Set(["", "text", "search", "email", "url", "tel", "password", "number",
    "date", "datetime-local", "month", "time", "week"]);

  // The element that really has the keyboard, inside web components too.
  function active() {
    let el = document.activeElement;
    while (el && el.shadowRoot && el.shadowRoot.activeElement) el = el.shadowRoot.activeElement;
    return el;
  }

  // Is `top` (found at a point) `el` itself, inside it, or what holds it?
  function same(top, el) {
    if (!top) return false;
    if (top === el || el.contains(top) || top.contains(el)) return true;
    for (let n = el.getRootNode(); n && n.host; n = n.host.getRootNode()) {
      if (n.host === top || top.contains(n.host)) return true;
    }
    return false;
  }

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
      const root = el.getRootNode();
      const top = (root.elementFromPoint ? root : document).elementFromPoint(x, y);
      if (same(top, el) || (top && el.tagName === "INPUT" && top.tagName === "LABEL" && top.control === el)) return r;
    }
    return null;
  }

  function targets() {
    const out = [];
    const seen = new Set();
    for (const el of everywhere()) {
      // a link wrapping a button counts once
      if (seen.has(el.parentElement) && el.closest("a[href],button") !== el) continue;
      const r = onScreen(el);
      if (r) { out.push({ el, r }); seen.add(el); }
    }
    return out;
  }

  // Everything clickable, also inside web components (YouTube is made of them).
  function everywhere() {
    const out = [];
    const walk = (root) => {
      for (const el of root.querySelectorAll(CLICKABLE)) out.push(el);
      for (const el of root.querySelectorAll("*")) if (el.shadowRoot) walk(el.shadowRoot);
    };
    walk(document);
    return out;
  }

  function press(el) {
    if (typing(el)) { el.focus(); return; }
    el.focus({ preventScroll: true });
    el.click();
  }

  // ------------------------------------------------------------ letters

  let hint = null; // { host, items: [{code, el, tag}], typed }
  // a text box the arrows moved into: until something is typed there, the
  // arrows (and letters) still move on
  let passing = null;

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
    const cur = active();
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

  // Letters, help and Esc: before the page, so no site can take them.
  window.addEventListener("keydown", (e) => {
    if (hint || help || e.ctrlKey || e.metaKey || e.isComposing) return;
    const el = active();
    // Alt . : letters from anywhere, even from inside a text box
    if (opt.letters && e.altKey && !e.shiftKey && (e.key === "." || e.code === "Period")) {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (typing(el)) el.blur();
      showHints();
      return;
    }
    if (e.altKey) return;
    // pages often put you in a search or chat box: Esc lets you out
    // (the page still sees the Esc, to close its own menus)
    if (e.key === "Escape" && opt.escape && typing(el)) {
      el.blur();
      passing = null;
      return;
    }
    const arrow = e.key.startsWith("Arrow");
    if (passing && passing !== el) passing = null;
    if (passing && !arrow && e.key.toLowerCase() !== (opt.letterKey || "f") && e.key !== "?") passing = null;
    if (typing(el) && !passing) return;
    if (opt.letters && e.key.toLowerCase() === (opt.letterKey || "f") && !e.shiftKey) {
      e.preventDefault();
      e.stopImmediatePropagation();
      showHints();
      return;
    }
    if (e.key === "?" && opt.help && window === window.top) {
      e.preventDefault();
      e.stopImmediatePropagation();
      toggleHelp();
    }
  }, true);

  // Arrows and Enter only when the page itself did nothing with the key.
  window.addEventListener("keydown", (e) => {
    if (hint || help || e.defaultPrevented || e.ctrlKey || e.altKey || e.metaKey) return;
    const el = active();
    const dir = opt.arrows ? { ArrowRight: "right", ArrowLeft: "left", ArrowDown: "down", ArrowUp: "up" }[e.key] : undefined;
    if (passing && passing !== el) passing = null;
    if (typing(el) && !passing && !(dir && canLeave(el, dir))) return;
    if (dir && !e.shiftKey) {
      const next = nearest(dir);
      if (next) {
        e.preventDefault();
        next.focus({ preventScroll: true });
        passing = typing(next) ? next : null;
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

  // ------------------------------------------------------------ help

  let help = null;
  function toggleHelp() {
    if (help) { help.remove(); help = null; return; }
    help = document.createElement("chlorum-keys-help");
    help.style.cssText = "all:initial;position:fixed;inset:0;z-index:2147483647;";
    const root = help.attachShadow({ mode: "closed" });
    const k = (opt.letterKey || "f").toUpperCase();
    root.innerHTML = `<style>
      .w{position:fixed;inset:0;background:rgba(0,0,0,.6);display:flex;align-items:center;justify-content:center}
      .b{background:#0b0b0b;color:#f5f5f5;border-top:4px solid #FFD60A;padding:28px 36px;min-width:480px;
         font:16px/1.5 "IBM Plex Sans",system-ui,sans-serif}
      h1{font:600 24px/1 "IBM Plex Sans",system-ui,sans-serif;margin:0 0 18px}
      td{padding:6px 18px 6px 0;vertical-align:top}
      kbd{font:600 13px "IBM Plex Mono",ui-monospace,monospace;border:1.5px solid #FFD60A;color:#FFD60A;padding:2px 7px}
      .off{opacity:.35}
      p{color:#9a9a9a;font-size:14px;margin:18px 0 0}</style>
      <div class="w"><div class="b"><h1>Keys on web pages</h1><table>
      <tr class="${opt.arrows ? "" : "off"}"><td><kbd>← ↑ ↓ →</kbd></td><td>Move to the next button or link</td></tr>
      <tr><td><kbd>Enter</kbd></td><td>Press it</td></tr>
      <tr class="${opt.letters ? "" : "off"}"><td><kbd>${k}</kbd></td><td>A letter on every button and link: type it to press</td></tr>
      <tr class="${opt.letters ? "" : "off"}"><td><kbd>Alt .</kbd></td><td>The same from anywhere, even while typing in a text box</td></tr>
      <tr class="${opt.escape ? "" : "off"}"><td><kbd>Esc</kbd></td><td>Leave a text box, so the arrows move again</td></tr>
      <tr><td><kbd>Ctrl L</kbd></td><td>Address bar · <kbd>Ctrl T</kbd> new tab · <kbd>Ctrl W</kbd> close tab</td></tr>
      <tr><td><kbd>Alt ←</kbd></td><td>Back · <kbd>Alt →</kbd> forward · <kbd>Ctrl Tab</kbd> next tab</td></tr>
      </table><p>Any key closes this. Turn keys on or off: Chlorum Keys in the toolbar.</p></div></div>`;
    document.documentElement.appendChild(help);
  }
  window.addEventListener("keydown", (e) => {
    if (!help) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    toggleHelp();
  }, true);

  window.addEventListener("scroll", hideHints, { passive: true });
  window.addEventListener("resize", hideHints, { passive: true });
})();
