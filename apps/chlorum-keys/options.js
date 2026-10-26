// The settings page: every change is saved at once; open pages pick it up.
const DEFAULTS = { arrows: true, letters: true, letterKey: "f", escape: true, help: true };
const $ = (id) => document.getElementById(id);
const saved = $("saved");
let timer;
function say(s) { saved.textContent = s; clearTimeout(timer); timer = setTimeout(() => (saved.textContent = ""), 1600); }

browser.storage.local.get(DEFAULTS).then((v) => {
  for (const k of ["arrows", "letters", "escape", "help"]) {
    $(k).checked = v[k];
    $(k).addEventListener("change", () => browser.storage.local.set({ [k]: $(k).checked }).then(() => say("Saved.")));
  }
  $("letterKey").value = v.letterKey;
  $("letterKey").addEventListener("input", () => {
    const c = $("letterKey").value.toLowerCase();
    if (/^[a-z]$/.test(c)) browser.storage.local.set({ letterKey: c }).then(() => say("Saved."));
  });
  $("arrows").focus();
});
