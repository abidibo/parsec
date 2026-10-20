// Parsec website: the warp starfield, the launcher demo, and small touches.
"use strict";

const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/* ------------------------------------------------------------ starfield */

(function warp() {
  const canvas = $(".warp");
  const ctx = canvas.getContext("2d");
  const stars = [];
  const COUNT = 420;
  let w = 0, h = 0, dpr = 1;
  let speed = reduceMotion ? 0 : 38; // Launch at warp speed, then cruise.
  let visible = true;
  let last = performance.now();

  function size() {
    dpr = Math.min(devicePixelRatio || 1, 2);
    w = canvas.clientWidth;
    h = canvas.clientHeight;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }
  function spawn(star, far) {
    star.x = (Math.random() - 0.5) * 2;
    star.y = (Math.random() - 0.5) * 2;
    star.z = far ? 1 : Math.random() * 0.95 + 0.05;
    star.pz = star.z;
    star.hue = Math.random() < 0.18 ? (Math.random() < 0.5 ? "139,124,255" : "106,212,255") : "255,255,255";
    return star;
  }
  for (let i = 0; i < COUNT; i++) stars.push(spawn({}, false));

  function frame(now) {
    const dt = Math.min((now - last) / 16.7, 3);
    last = now;
    ctx.clearRect(0, 0, w, h);
    const cx = w / 2, cy = h * 0.42, scale = Math.max(w, h) * 0.6;
    speed += (0.55 - speed) * 0.035 * dt;
    for (const s of stars) {
      s.pz = s.z;
      s.z -= 0.0016 * speed * dt;
      if (s.z <= 0.02) { spawn(s, true); continue; }
      const x = cx + (s.x / s.z) * scale, y = cy + (s.y / s.z) * scale;
      const px = cx + (s.x / s.pz) * scale, py = cy + (s.y / s.pz) * scale;
      if (x < -50 || x > w + 50 || y < -50 || y > h + 50) { spawn(s, true); continue; }
      const alpha = Math.min(1, (1 - s.z) * 1.4);
      const r = (1 - s.z) * 1.6 + 0.2;
      ctx.strokeStyle = `rgba(${s.hue},${alpha})`;
      ctx.lineWidth = r;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(px, py);
      ctx.lineTo(x + 0.01, y);
      ctx.stroke();
    }
    if (visible && !reduceMotion) requestAnimationFrame(frame);
  }

  size();
  addEventListener("resize", size);
  if (reduceMotion) {
    frame(performance.now());
    return;
  }
  new IntersectionObserver(([e]) => {
    const was = visible;
    visible = e.isIntersecting;
    if (visible && !was) { last = performance.now(); requestAnimationFrame(frame); }
  }).observe(canvas);
  requestAnimationFrame(frame);
})();

/* ------------------------------------------------- desktop inside screen */

(function fit() {
  const screen = $(".screen");
  const desktop = $(".desktop");
  new ResizeObserver(() => {
    desktop.style.setProperty("--s", screen.clientWidth / 1280);
  }).observe(screen);
})();

/* -------------------------------------------------------- launcher demo */

const NVIM = `<svg class="nvim" viewBox="0 0 24 24"><path d="M3 6.5 8 2v20l-5-4.5z" fill="#4fa94d"/><path d="M21 6.5 16 2v20l5-4.5z" fill="#3a8fd8"/><path d="M8 2l10 18-2 2L6 4z" fill="#62c85d" opacity=".85"/></svg>`;
const QR_PATH = "M0 0h7v1h-7zM8 0h2v1h-2zM11 0h2v1h-2zM14 0h1v1h-1zM17 0h2v1h-2zM22 0h7v1h-7zM0 1h1v1h-1zM6 1h1v1h-1zM9 1h2v1h-2zM14 1h1v1h-1zM16 1h1v1h-1zM18 1h1v1h-1zM20 1h1v1h-1zM22 1h1v1h-1zM28 1h1v1h-1zM0 2h1v1h-1zM2 2h3v1h-3zM6 2h1v1h-1zM8 2h2v1h-2zM11 2h5v1h-5zM17 2h4v1h-4zM22 2h1v1h-1zM24 2h3v1h-3zM28 2h1v1h-1zM0 3h1v1h-1zM2 3h3v1h-3zM6 3h1v1h-1zM9 3h1v1h-1zM13 3h2v1h-2zM16 3h4v1h-4zM22 3h1v1h-1zM24 3h3v1h-3zM28 3h1v1h-1zM0 4h1v1h-1zM2 4h3v1h-3zM6 4h1v1h-1zM10 4h3v1h-3zM14 4h1v1h-1zM18 4h1v1h-1zM22 4h1v1h-1zM24 4h3v1h-3zM28 4h1v1h-1zM0 5h1v1h-1zM6 5h1v1h-1zM8 5h3v1h-3zM14 5h1v1h-1zM22 5h1v1h-1zM28 5h1v1h-1zM0 6h7v1h-7zM8 6h1v1h-1zM10 6h1v1h-1zM12 6h1v1h-1zM14 6h1v1h-1zM16 6h1v1h-1zM18 6h1v1h-1zM20 6h1v1h-1zM22 6h7v1h-7zM9 7h2v1h-2zM12 7h1v1h-1zM18 7h2v1h-2zM0 8h1v1h-1zM2 8h1v1h-1zM6 8h2v1h-2zM9 8h2v1h-2zM14 8h2v1h-2zM19 8h2v1h-2zM23 8h1v1h-1zM26 8h1v1h-1zM28 8h1v1h-1zM0 9h2v1h-2zM3 9h3v1h-3zM11 9h1v1h-1zM13 9h1v1h-1zM15 9h2v1h-2zM19 9h5v1h-5zM27 9h2v1h-2zM1 10h1v1h-1zM5 10h2v1h-2zM11 10h2v1h-2zM15 10h1v1h-1zM18 10h2v1h-2zM22 10h5v1h-5zM28 10h1v1h-1zM1 11h1v1h-1zM5 11h1v1h-1zM8 11h1v1h-1zM12 11h1v1h-1zM14 11h3v1h-3zM21 11h1v1h-1zM24 11h2v1h-2zM4 12h3v1h-3zM8 12h1v1h-1zM10 12h1v1h-1zM14 12h1v1h-1zM16 12h1v1h-1zM19 12h2v1h-2zM22 12h2v1h-2zM28 12h1v1h-1zM2 13h2v1h-2zM7 13h2v1h-2zM12 13h1v1h-1zM14 13h2v1h-2zM17 13h1v1h-1zM19 13h2v1h-2zM22 13h2v1h-2zM27 13h2v1h-2zM0 14h1v1h-1zM2 14h1v1h-1zM4 14h3v1h-3zM8 14h2v1h-2zM13 14h1v1h-1zM15 14h1v1h-1zM19 14h1v1h-1zM21 14h4v1h-4zM28 14h1v1h-1zM0 15h4v1h-4zM5 15h1v1h-1zM7 15h1v1h-1zM15 15h1v1h-1zM18 15h3v1h-3zM22 15h2v1h-2zM1 16h1v1h-1zM5 16h2v1h-2zM9 16h1v1h-1zM12 16h1v1h-1zM14 16h3v1h-3zM18 16h1v1h-1zM20 16h3v1h-3zM28 16h1v1h-1zM3 17h3v1h-3zM7 17h1v1h-1zM10 17h1v1h-1zM14 17h4v1h-4zM19 17h1v1h-1zM22 17h2v1h-2zM26 17h3v1h-3zM0 18h4v1h-4zM6 18h3v1h-3zM11 18h1v1h-1zM14 18h2v1h-2zM19 18h1v1h-1zM24 18h2v1h-2zM28 18h1v1h-1zM2 19h1v1h-1zM4 19h1v1h-1zM7 19h3v1h-3zM11 19h2v1h-2zM16 19h1v1h-1zM18 19h1v1h-1zM20 19h1v1h-1zM22 19h1v1h-1zM24 19h1v1h-1zM0 20h5v1h-5zM6 20h2v1h-2zM9 20h1v1h-1zM13 20h4v1h-4zM20 20h6v1h-6zM27 20h1v1h-1zM8 21h1v1h-1zM11 21h1v1h-1zM13 21h1v1h-1zM15 21h2v1h-2zM18 21h1v1h-1zM20 21h1v1h-1zM24 21h3v1h-3zM28 21h1v1h-1zM0 22h7v1h-7zM8 22h2v1h-2zM11 22h2v1h-2zM15 22h1v1h-1zM17 22h2v1h-2zM20 22h1v1h-1zM22 22h1v1h-1zM24 22h1v1h-1zM28 22h1v1h-1zM0 23h1v1h-1zM6 23h1v1h-1zM10 23h2v1h-2zM13 23h1v1h-1zM15 23h1v1h-1zM19 23h2v1h-2zM24 23h1v1h-1zM27 23h1v1h-1zM0 24h1v1h-1zM2 24h3v1h-3zM6 24h1v1h-1zM12 24h2v1h-2zM15 24h1v1h-1zM19 24h7v1h-7zM27 24h1v1h-1zM0 25h1v1h-1zM2 25h3v1h-3zM6 25h1v1h-1zM10 25h1v1h-1zM13 25h1v1h-1zM15 25h2v1h-2zM19 25h3v1h-3zM24 25h3v1h-3zM28 25h1v1h-1zM0 26h1v1h-1zM2 26h3v1h-3zM6 26h1v1h-1zM8 26h2v1h-2zM13 26h6v1h-6zM21 26h1v1h-1zM24 26h1v1h-1zM27 26h2v1h-2zM0 27h1v1h-1zM6 27h1v1h-1zM15 27h1v1h-1zM18 27h1v1h-1zM20 27h1v1h-1zM23 27h3v1h-3zM0 28h7v1h-7zM8 28h2v1h-2zM11 28h1v1h-1zM15 28h1v1h-1zM20 28h1v1h-1zM24 28h2v1h-2zM28 28h1v1h-1z";
const QR = `<div class="qr"><svg viewBox="0 0 29 29" shape-rendering="crispEdges"><path d="${QR_PATH}" fill="#000"/></svg></div>`;

const icon = (name) => (name.startsWith("<") ? name : `<svg><use href="#i-${name}"/></svg>`);

/** `title` with the letters of `query` marked, matched in order; null when they aren't all there. */
function fuzzy(title, query) {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return esc(title);
  let out = "", i = 0;
  for (const ch of title) {
    if (i < q.length && ch.toLowerCase() === q[i]) { out += `<b>${esc(ch)}</b>`; i++; }
    else out += esc(ch);
  }
  return i === q.length ? out.replace(/<\/b><b>/g, "") : null;
}

class Cancelled extends Error {}

const demo = (() => {
  const pz = $("#pz");
  const els = {
    stages: $(".pz-stages", pz),
    chip: $(".pz-chip", pz),
    input: $(".pz-input", pz),
    text: $(".pz-text", pz),
    results: $(".pz-results", pz),
    preview: $(".pz-preview-inner", pz),
    pipeHint: $(".hint-pipe", pz),
    runHint: $(".pz-hints span:nth-child(2)", pz),
  };
  const device = $(".device");
  const tabs = $$(".scenes button");
  let run = 0;          // bumped to cancel the scene that's playing
  let onScreen = true;
  let instant = reduceMotion;
  let rows = [];        // rows currently shown, for selection

  const visible = () => onScreen && !document.hidden;
  function setPaused() { $(".scenes").classList.toggle("paused", !visible()); }
  new IntersectionObserver(([e]) => { onScreen = e.isIntersecting; setPaused(); }, { threshold: 0.25 }).observe(device);
  document.addEventListener("visibilitychange", setPaused);

  /** Wait `ms` of time the demo is actually on screen; throws if a new scene took over. */
  async function wait(ms, token) {
    if (instant) { check(token); return; }
    let left = ms;
    while (left > 0) {
      const step = Math.min(left, 100);
      await new Promise((r) => setTimeout(r, step));
      check(token);
      if (visible()) left -= step;
    }
  }
  function check(token) { if (token !== run) throw new Cancelled(); }

  function setText(t) {
    els.text.textContent = t;
    els.input.classList.toggle("filled", t.length > 0 || !els.chip.hidden || els.stages.children.length > 0);
  }
  function setChip(label) {
    els.chip.hidden = !label;
    els.chip.textContent = label || "";
    setText(els.text.textContent);
  }
  async function type(str, token, onEach) {
    for (const ch of str) {
      setText(els.text.textContent + ch);
      onEach?.(els.text.textContent);
      await wait(55 + Math.random() * 70, token);
    }
  }
  function clear() {
    els.stages.innerHTML = "";
    setChip(null);
    setText("");
    showRows([]);
    closePreview();
    els.pipeHint.classList.remove("flash");
    els.runHint.classList.remove("flash");
  }

  /** Rows grouped under their section headers, the `sel`th one selected. */
  function showRows(list, sel = 0) {
    rows = list;
    let html = "", section = null;
    list.forEach((r, i) => {
      if (r.section && r.section !== section) { html += `<div class="pz-section">${r.section}</div>`; section = r.section; }
      const key = r.key ? `<kbd>${r.key}</kbd>` : "";
      if (r.picture) {
        html += `<div class="pz-row picture ${i === sel ? "sel" : ""}">${r.picture}<div class="pz-texts"><div class="pz-title">${r.html ?? esc(r.title)}</div><div class="pz-sub">${esc(r.sub)}</div></div><div class="pz-action">${r.action}${key}</div></div>`;
      } else {
        html += `<div class="pz-row ${i === sel ? "sel" : ""}"><div class="pz-tile t-${r.tile}">${icon(r.icon)}</div><div class="pz-texts"><div class="pz-title">${r.html ?? esc(r.title)}</div><div class="pz-sub">${esc(r.sub || "")}</div></div><div class="pz-action">${r.action}${key}</div></div>`;
      }
    });
    els.results.innerHTML = html;
  }
  function select(i) {
    $$(".pz-row", els.results).forEach((el, j) => el.classList.toggle("sel", i === j));
  }
  function notice(text) {
    rows = [];
    els.results.innerHTML = `<div class="pz-section" style="padding:120px 0;text-align:center;letter-spacing:0;text-transform:none;font-size:14px;font-weight:400">${text}</div>`;
  }
  /** Filter `candidates` by what's typed, like the launcher does. */
  const live = (candidates) => (q) => {
    showRows(candidates.map((c) => ({ ...c, html: fuzzy(c.title, q) })).filter((c) => c.html !== null));
  };

  function openPreview(html) { els.preview.innerHTML = html; pz.classList.add("wide"); }
  function closePreview() { pz.classList.remove("wide"); }
  async function flash(hint, token) {
    hint.classList.add("flash");
    await wait(650, token);
    hint.classList.remove("flash");
  }
  const facts = (list) =>
    `<dl class="facts-grid">${list.map(([k, v]) => `<dt>${k}</dt><dd>${esc(v)}</dd>`).join("")}</dl>`;

  /* ---- the scenes ---- */

  async function search(t) {
    const candidates = [
      { section: "APPS", tile: "apps", icon: NVIM, title: "Neovim", sub: "Text Editor", action: "Launch" },
      { section: "PROJECTS", tile: "projects", icon: "folder", title: "neo-dashboard", sub: "~/Dev/neo-dashboard", action: "Open in editor", key: "⇥" },
      { section: "GITHUB", tile: "github", icon: "repo", title: "abidibo/neon-weather", sub: "A neon weather widget for GNOME · ★ 128", action: "Open", key: "⇥" },
      { section: "FILES", tile: "files", icon: "image", title: "neon-poster.png", sub: "~/Pictures/neon-poster.png · 2.4 MB", action: "Open" },
      { section: "CLIPBOARD", tile: "clipboard", icon: "clipboard", title: "neofetch --ascii_distro ubuntu", sub: "Copied 5 minutes ago", action: "Copy" },
    ];
    await wait(700, t);
    await type("neo", t, live(candidates));
    await wait(1400, t);
    for (const i of [1, 2, 3, 0]) { select(i); await wait(700, t); }
  }

  const PARSEC_README = `<div class="md"><h1>Parsec</h1><p>A personal launcher for GNOME. Press a key, type, press Enter.</p><p>Apps, your git projects with their branches, commits and files, a shell runner, GitHub repositories and pull requests, clipboard history with pinned snippets, KeePass entries, files, SSH hosts…</p><ul><li>Ctrl+Enter pipes a result into the next verb</li><li>Quick Look previews what you select</li></ul><p><b>Rust, GTK 4, libadwaita.</b> Wayland first.</p></div>`;
  const ASK_README = `<div class="md"><h1>Ask: Claude in the Parsec launcher</h1><p>Ask Claude a quick question, or fix the text you just copied, without leaving what you're doing.</p><ul><li><code>ask how do I…</code> streams the answer</li><li><code>fix</code> rewrites the copied text</li></ul><p>It runs <b>Claude Code</b> headless, so it uses your Claude plan.</p></div>`;

  async function quickLook(t) {
    const candidates = [
      { section: "PROJECTS", tile: "projects", icon: "folder", title: "parsec", sub: "~/Dev/abidibo/parsec", action: "Open in editor", key: "⇥" },
      { section: "PROJECTS", tile: "projects", icon: "folder", title: "parsec-ask", sub: "~/Dev/abidibo/parsec-ask", action: "Open in editor", key: "⇥" },
      { section: "PROJECTS", tile: "projects", icon: "folder", title: "parsec-voice", sub: "~/Dev/abidibo/parsec-voice", action: "Open in editor", key: "⇥" },
      { section: "GITHUB", tile: "github", icon: "repo", title: "abidibo/parsec", sub: "Launch at warp speed · ★ 342", action: "Open", key: "⇥" },
    ];
    await wait(600, t);
    await type("parsec", t, live(candidates));
    await wait(500, t);
    openPreview(PARSEC_README + facts([["Branch", "dev"], ["Last commit", "chore: release 1.10.0 · 2 hours ago"], ["Changes", "clean"], ["Path", "~/Dev/abidibo/parsec"]]));
    await wait(3000, t);
    select(1);
    openPreview(ASK_README + facts([["Branch", "main"], ["Last commit", "feat: answers in the preview pane · 1 day ago"], ["Changes", "clean"], ["Path", "~/Dev/abidibo/parsec-ask"]]));
    await wait(2600, t);
  }

  async function pipes(t) {
    await wait(600, t);
    await type("pr", t);
    await wait(250, t);
    setText("");
    setChip("pr");
    showRows([
      { section: "PULL REQUESTS", tile: "github", icon: "pr", title: "#42 feat(updates): one-click Parsec and plugin updates", sub: "abidibo/parsec · checks passed · 2 hours ago", action: "Open", key: "⇥" },
      { section: "PULL REQUESTS", tile: "github", icon: "pr", title: "#7 feat: answers rendered as Markdown", sub: "abidibo/parsec-ask · review requested", action: "Open", key: "⇥" },
      { section: "PULL REQUESTS", tile: "github", icon: "pr", title: "#3 fix: Whisper model download progress", sub: "abidibo/parsec-voice · draft", action: "Open", key: "⇥" },
    ]);
    await wait(1600, t);
    await flash(els.pipeHint, t);
    setChip(null);
    els.stages.innerHTML = `<span class="pz-stage">pr #42</span><span class="pz-pipe">›</span>`;
    setText("");
    notice("Type a verb to pipe into: qr, ask, $ …");
    await wait(900, t);
    await type("qr", t);
    await wait(300, t);
    showRows([{ picture: QR, title: "github.com/abidibo/parsec/pull/42", sub: "QR code of the pull request's address", action: "Copy image", key: "⇥" }]);
    await wait(3200, t);
  }

  const ANSWER = [
    ["p", "The safe way is to revert, which adds a new commit that undoes the old one:"],
    ["pre", "git revert <commit-hash>\ngit push"],
    ["p", "Use ", ["code", "git revert HEAD"], " for the most recent commit."],
    ["p", "Only rewrite history if nobody else has pulled it:"],
    ["pre", "git reset --hard HEAD~1\ngit push --force-with-lease"],
  ];

  async function plugins(t) {
    await wait(600, t);
    await type("ask ", t);
    setText("");
    setChip("ask");
    const question = "how do I undo a pushed commit";
    await type(question, t, (q) =>
      showRows([{ tile: "plugins", icon: "question", title: `Ask: ${q}`, sub: "Enter asks Claude Sonnet", action: "Answer" }]));
    await wait(700, t);
    await flash(els.runHint, t);
    setChip("Answer");
    setText("");
    notice("Thinking…");
    await wait(1100, t);
    const row = { tile: "plugins", icon: "question", title: "Claude Sonnet · writing…", sub: "", action: "" };
    showRows([row]);
    openPreview(`<div class="md"></div>`);
    const md = $(".md", els.preview);
    let first = "";
    for (const [tag, ...parts] of ANSWER) {
      const block = document.createElement(tag);
      md.append(block);
      for (const part of parts) {
        const target = Array.isArray(part) ? block.appendChild(document.createElement(part[0])) : block;
        const text = Array.isArray(part) ? part[1] : part;
        const node = target.appendChild(document.createTextNode(""));
        for (const word of text.split(/(?<=\s)/)) {
          node.data += word;
          block.classList.add("typing");
          await wait(tag === "pre" ? 60 : 38, t);
        }
      }
      block.classList.remove("typing");
      if (!first) {
        first = block.textContent;
        $(".pz-sub", els.results).textContent = first.slice(0, 64) + "…";
      }
    }
    showRows([{ tile: "plugins", icon: "question", title: "Claude Sonnet", sub: first.slice(0, 64) + "…", action: "Copy", key: "⇥" }]);
    await wait(3200, t);
  }

  const scenes = [search, quickLook, pipes, plugins];
  const durations = [7600, 10200, 11000, 13500];
  let current = 0;

  async function play(i) {
    const token = ++run;
    current = i;
    tabs.forEach((b, j) => {
      b.setAttribute("aria-selected", String(i === j));
      b.style.setProperty("--d", `${durations[j]}ms`);
      // Restart the progress bar.
      const bar = $("i", b);
      bar.style.animation = "none";
      void bar.offsetWidth;
      bar.style.animation = "";
    });
    clear();
    const started = performance.now();
    try {
      await scenes[i](token);
      if (instant) return;
      await wait(Math.max(0, durations[i] - (performance.now() - started)), token);
      play((i + 1) % scenes.length);
    } catch (e) {
      if (!(e instanceof Cancelled)) throw e;
    }
  }

  tabs.forEach((b, i) => b.addEventListener("click", () => play(i)));
  return { start: () => play(reduceMotion ? 1 : 0) };
})();
demo.start();

/* ---------------------------------------------------------------- bits */

$$("[data-copy]").forEach((box) => {
  const button = $(".copy", box);
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(box.dataset.copy);
    } catch {
      return;
    }
    button.classList.add("done");
    $("use", button).setAttribute("href", "#i-check");
    setTimeout(() => {
      button.classList.remove("done");
      $("use", button).setAttribute("href", "#i-copy");
    }, 1600);
  });
});

$$(".cw-tabs button").forEach((tab) => {
  tab.addEventListener("click", () => {
    $$(".cw-tabs button").forEach((b) => b.classList.toggle("on", b === tab));
    $$(".cw-file").forEach((f, i) => f.classList.toggle("on", String(i) === tab.dataset.file));
  });
});

// The latest release, so the version never goes stale.
fetch("https://api.github.com/repos/abidibo/parsec/releases/latest")
  .then((r) => (r.ok ? r.json() : null))
  .then((release) => {
    if (!release?.tag_name) return;
    const v = $(".kicker .version");
    v.textContent = `v${release.tag_name.replace(/^v/, "")}`;
    v.hidden = false;
  })
  .catch(() => {});

if (!reduceMotion) {
  const items = $$(".section-head, .verb, .card, .keys, .plugins-copy, .code-window, .install-card");
  items.forEach((el) => {
    el.classList.add("reveal");
    const siblings = [...el.parentElement.children].filter((c) => c.classList.contains(el.classList[0]));
    el.style.transitionDelay = `${Math.min(siblings.indexOf(el), 8) * 45}ms`;
  });
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
  }, { rootMargin: "0px 0px -8% 0px" });
  items.forEach((el) => io.observe(el));
}
