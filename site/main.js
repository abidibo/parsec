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
    placeholder: $(".pz-placeholder", pz),
    tabHint: $(".pz-hints span:nth-child(1)", pz),
    runHint: $(".pz-hints span:nth-child(2)", pz),
    pipeHint: $(".hint-pipe", pz),
    toast: $(".toast"),
  };
  const PLACEHOLDER = els.placeholder.textContent;
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
  function setPlaceholder(text) { els.placeholder.textContent = text ?? PLACEHOLDER; }
  function showToast() { els.toast.hidden = false; void els.toast.offsetWidth; els.toast.classList.add("on"); }
  function clear() {
    pz.classList.remove("gone");
    els.input.classList.remove("prompt");
    setPlaceholder(null);
    els.toast.classList.remove("on");
    els.toast.hidden = true;
    for (const hint of [els.tabHint, els.runHint, els.pipeHint]) hint.classList.remove("flash");
    els.stages.innerHTML = "";
    setChip(null);
    setText("");
    showRows([]);
    closePreview();
  }

  /** Rows grouped under their section headers, the `sel`th one selected. */
  function showRows(list, sel = 0) {
    rows = list;
    let html = "", section = null;
    list.forEach((r, i) => {
      if (r.section && r.section !== section) { html += `<div class="pz-section">${r.section}</div>`; section = r.section; }
      const key = r.key ? `<kbd>${r.key}</kbd>` : "";
      html += `<div class="pz-row ${i === sel ? "sel" : ""}"><div class="pz-tile t-${r.tile}">${icon(r.icon)}</div><div class="pz-texts"><div class="pz-title">${r.html ?? esc(r.title)}</div><div class="pz-sub">${esc(r.sub || "")}</div></div><div class="pz-action">${r.action || ""}${key}</div></div>`;
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
    showRows(candidates
      .map((c) => ({ ...c, html: fuzzy(c.title, q) ?? (c.always ? esc(c.title) : null) }))
      .filter((c) => c.html !== null));
  };

  function openPreview(html) { els.preview.innerHTML = html; pz.classList.add("wide"); }
  function closePreview() { pz.classList.remove("wide"); }
  async function flash(hint, token, ms = 650) {
    hint.classList.add("flash");
    await wait(ms, token);
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

  /** Stream `blocks` ([tag, ...parts], a part being text or [tag, text]) into `md` word by word. */
  async function stream(md, blocks, t, onFirst) {
    let first = "";
    for (const [tag, ...parts] of blocks) {
      const block = md.appendChild(document.createElement(tag));
      block.classList.add("typing");
      for (const part of parts) {
        const target = Array.isArray(part) ? block.appendChild(document.createElement(part[0])) : block;
        const node = target.appendChild(document.createTextNode(""));
        for (const word of (Array.isArray(part) ? part[1] : part).split(/(?<=\s)/)) {
          node.data += word;
          await wait(tag === "pre" ? 55 : 34, t);
        }
      }
      block.classList.remove("typing");
      if (!first) onFirst?.((first = block.textContent));
    }
    return first;
  }

  const LOGS = [
    "09:38:12.402 UTC [1] LOG:  starting PostgreSQL 17.2 on x86_64-pc-linux-musl",
    "09:38:12.405 UTC [1] LOG:  listening on IPv4 address \"0.0.0.0\", port 5432",
    "09:38:12.411 UTC [1] FATAL:  could not map anonymous shared memory: Cannot allocate memory",
    "09:38:12.411 UTC [1] HINT:  This error usually means that PostgreSQL's request for a shared memory segment exceeded available memory, swap space, or huge pages. To reduce the request size (currently 4404486144 bytes), reduce PostgreSQL's shared memory usage, perhaps by reducing shared_buffers or max_connections.",
    "09:38:12.413 UTC [1] LOG:  database system is shut down",
  ];
  const DIAGNOSIS = [
    ["p", "Postgres asks for ", ["b", "4.1 GB"], " of shared memory at startup, more than the container may use, so it exits and Docker restarts it in a loop."],
    ["p", "Lower ", ["code", "shared_buffers"], " to fit the limit:"],
    ["pre", "command: postgres -c shared_buffers=512MB"],
    ["p", "or give the container more memory:"],
    ["pre", "deploy:\n  resources:\n    limits:\n      memory: 6g"],
  ];

  async function pipes(t) {
    const LOGS_CMD = "$ docker logs --tail 80 {}";
    await wait(600, t);
    await type("dk ", t);
    setText("");
    setChip("dk");
    const containers = [
      { section: "DOCKER", tile: "docker", icon: "box", title: "postgres", sub: "postgres:17-alpine · Restarting (1) 4 seconds ago", action: "Logs", key: "⇥" },
      { section: "DOCKER", tile: "docker", icon: "box", title: "api", sub: "ghcr.io/acme/api:2.3 · Up 3 hours", action: "Shell", key: "⇥" },
      { section: "DOCKER", tile: "docker", icon: "box", title: "redis", sub: "redis:7 · Up 3 hours", action: "Shell", key: "⇥" },
    ];
    live(containers)("");
    await wait(500, t);
    await type("pg", t, live(containers));
    await wait(900, t);

    // Stage 1: the container hands on its id.
    await flash(els.pipeHint, t);
    setChip(null);
    els.stages.innerHTML = `<span class="pz-stage">postgres</span><span class="pz-pipe">›</span>`;
    setText("");
    notice("Type a verb to pipe into");
    await wait(700, t);
    await type(LOGS_CMD, t, (q) =>
      showRows([{ tile: "shell", icon: "terminal", title: q, sub: "Ctrl+Enter runs it and pipes its output", action: "Run in terminal" }]));
    await wait(700, t);

    // Stage 2: the command runs in the background and hands on its output.
    await flash(els.pipeHint, t);
    els.stages.innerHTML += `<span class="pz-stage">${esc(LOGS_CMD.replace("{}", "postgres"))}</span><span class="pz-pipe">›</span>`;
    setText("");
    notice("Running…");
    await wait(700, t);
    showRows([{ tile: "pipe", icon: "pipe", title: "Input from $ docker logs", sub: "80 lines · 6,214 characters · type a verb to pipe into", action: "Copy" }]);
    openPreview(`<pre class="logs">${LOGS.map(esc).join("\n")}</pre>`);
    await wait(2400, t);

    // Stage 3: Claude reads the logs.
    await type("ask ", t);
    setText("");
    setChip("ask");
    closePreview();
    await type("why does it crash?", t, (q) =>
      showRows([{ tile: "plugins", icon: "question", title: `Ask: ${q}`, sub: "Enter asks Claude Sonnet, with the logs", action: "Answer" }]));
    await wait(600, t);
    await flash(els.runHint, t);
    setChip("Answer");
    setText("");
    notice("Thinking…");
    await wait(1000, t);
    showRows([{ tile: "plugins", icon: "question", title: "Claude Sonnet · writing…" }]);
    openPreview(`<div class="md"></div>`);
    const first = await stream($(".md", els.preview), DIAGNOSIS, t, (text) => {
      $(".pz-sub", els.results).textContent = text.slice(0, 70) + "…";
    });
    showRows([{ tile: "plugins", icon: "question", title: "Claude Sonnet", sub: first.slice(0, 70) + "…", action: "Copy", key: "⇥" }]);
    await wait(3400, t);
  }

  async function passwords(t) {
    await wait(600, t);
    await type("kp ", t);
    setText("");
    setChip("kp");
    showRows([{ section: "KEEPASS", tile: "keepass", icon: "key", title: "Unlock Passwords.kdbx", sub: "~/Documents/Passwords.kdbx", action: "Unlock" }]);
    await wait(1100, t);
    await flash(els.runHint, t);

    // The box becomes a masked password field.
    setChip(null);
    els.input.classList.add("prompt");
    setPlaceholder("Master password for Passwords.kdbx");
    showRows([]);
    await wait(500, t);
    await type("••••••••••••••", t);
    await wait(400, t);
    await flash(els.runHint, t);
    els.input.classList.remove("prompt");
    setPlaceholder(null);
    setText("");
    setChip("kp");
    notice("Unlocking…");
    await wait(800, t);

    const lock = { section: "KEEPASS", tile: "keepass", icon: "key", title: "Lock database", sub: "Forget the decrypted entries now", action: "Lock", always: true };
    const entries = [
      { section: "KEEPASS", tile: "keepass", icon: "key", title: "GitHub", sub: "abidibo · Dev", action: "Copy password", key: "⇥" },
      { section: "KEEPASS", tile: "keepass", icon: "key", title: "GitHub Enterprise", sub: "s.contini · Work", action: "Copy password", key: "⇥" },
      { section: "KEEPASS", tile: "keepass", icon: "key", title: "Hetzner Cloud", sub: "abidibo · Servers", action: "Copy password", key: "⇥" },
      { section: "KEEPASS", tile: "keepass", icon: "key", title: "Postgres prod", sub: "admin · Servers", action: "Copy password", key: "⇥" },
      lock,
    ];
    live(entries)("");
    await wait(900, t);
    await type("github", t, live(entries));
    await wait(800, t);

    // Tab cycles the row's actions.
    const action = $(".pz-row.sel .pz-action", els.results);
    for (const label of ["Copy username", "Open URL", "Copy password"]) {
      await flash(els.tabHint, t, 450);
      action.firstChild.data = label;
      await wait(500, t);
    }
    await wait(300, t);
    await flash(els.runHint, t);

    // Enter copies it and the launcher goes away.
    pz.classList.add("gone");
    await wait(450, t);
    showToast();
    await wait(4200, t);
  }

  const scenes = [search, quickLook, pipes, passwords];
  const durations = [7600, 10200, 21000, 17500];
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
