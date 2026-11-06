// Countersign's web page: the sentry, the quick access demo and the themes.
(() => {
  'use strict';
  const $ = (s, r = document) => r.querySelector(s);
  const root = document.documentElement;
  const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
  const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
  const TAU = Math.PI * 2;

  /* ---------- Colors, ported from crates/countersign/src/theme.rs ---------- */
  const rgb = h => { h = h.replace('#', ''); return [0, 2, 4].map(i => parseInt(h.slice(i, i + 2), 16)); };
  const hex = c => '#' + c.map(v => Math.round(v).toString(16).padStart(2, '0')).join('');
  const mix = (a, b, t) => { const x = rgb(a), y = rgb(b); return hex(x.map((v, i) => v + (y[i] - v) * t)); };
  const lum = h => { const [r, g, b] = rgb(h).map(v => { v /= 255; return v <= .03928 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }); return .2126 * r + .7152 * g + .0722 * b; };
  const contrast = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + .05) / (Math.min(x, y) + .05); };
  const readable = (c, bgs, toward, min = 4.5) => {
    for (let i = 0; i <= 20; i++) { const m = mix(c, toward, i / 20); if (bgs.every(b => contrast(m, b) >= min)) return m; }
    return toward;
  };

  const NAMES = { 'rose-pine': 'Rosé Pine', 'retro-82': 'Retro 82', inkypinky: 'InkyPinky' };
  const title = s => NAMES[s] || s.split('-').map(w => w[0].toUpperCase() + w.slice(1)).join(' ');
  let themeIndex = -1;
  const VARS = ['--bg', '--raise', '--deep', '--fg', '--dim', '--muted', '--accent', '--accent-text', '--on-accent', '--sel', '--sel-fg', '--ok', '--warn', '--tone0', '--tone1', '--tone2', '--tone3', '--tone4', '--tone5', 'color-scheme'];

  function setTheme(i) {
    themeIndex = i;
    const t = THEMES[i];
    if (!t) {
      VARS.forEach(v => root.style.removeProperty(v));
    } else {
      const dark = t.m !== 'light';
      const page = dark ? t.bx : t.bd;
      const surfaces = [page, t.bg, t.bd, t.bl, t.bx];
      const away = dark ? '#ffffff' : '#000000';
      const onAccent = [t.bx, '#ffffff', '#000000'].sort((a, b) => contrast(b, t.a) - contrast(a, t.a))[0];
      const vars = {
        '--bg': page, '--raise': t.bg, '--deep': dark ? t.bx : t.bd,
        '--fg': t.fg, '--dim': readable(t.fl || t.fg, surfaces, t.fg), '--muted': readable(t.mu, surfaces, t.fg),
        '--accent': t.a, '--accent-text': readable(t.a, surfaces, away), '--on-accent': onAccent,
        '--sel': t.sel, '--sel-fg': readable(t.fg, [t.sel], away),
        '--ok': readable(t.gr, surfaces, away, 3), '--warn': readable(t.y, surfaces, away, 3),
        '--tone0': readable(t.b, [t.bg], away, 3), '--tone1': readable(t.c, [t.bg], away, 3), '--tone2': readable(t.gr, [t.bg], away, 3),
        '--tone3': readable(t.ma, [t.bg], away, 3), '--tone4': readable(t.o || t.y, [t.bg], away, 3), '--tone5': readable(t.r, [t.bg], away, 3),
        'color-scheme': dark ? 'dark' : 'light',
      };
      for (const k in vars) root.style.setProperty(k, vars[k]);
    }
    $('#theme-name').textContent = t ? title(t.s) : 'Sentry';
    document.querySelectorAll('.th').forEach((b, n) => b.setAttribute('aria-pressed', String(n === i)));
    sentry.readColors();
  }

  function wipeTo(i, x, y) {
    if (!document.startViewTransition || reduced()) return setTheme(i);
    try {
      const vt = document.startViewTransition(() => setTheme(i));
      const r = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
      vt.ready.then(() => root.animate(
        { clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${r}px at ${x}px ${y}px)`] },
        { duration: 620, easing: 'cubic-bezier(.22,1,.36,1)', pseudoElement: '::view-transition-new(root)' },
      )).catch(() => {});
    } catch { setTheme(i); }
  }

  /* ---------- The sentry: a halftone point cloud of the mascot ---------- */
  const sentry = (() => {
    const canvas = $('#sentry'), ctx = canvas.getContext('2d');
    const hero = $('.hero'), qa = $('#qa');
    const NB = 6, GROUPS = 3;
    let W = 0, H = 0, dpr = 1, s = 3.6, cx = 0, ground = 0, headY = 0, wide = false;
    let head = [], ears = [], body = [], dome = [];
    let col = {}, tiles = [];
    let running = false, visible = true, raf = 0, last = 0, t0 = 0;
    const st = { yaw: 0, yawV: 0, pitch: 0, pitchV: 0, sink: 0, sinkV: 0, hop: 0, hopV: 0, nod: 0, nodV: 0, blink: 0 };
    const tgt = { yaw: 0, pitch: .05, sink: 0, k: 70 };
    let pointerAt = -1e9, typingAt = -1e9, nextSaccade = 0, nextBlink = 2500, px = 0, py = 0;
    const buckets = Array.from({ length: NB * GROUPS }, () => ({ n: 0, a: new Float32Array(9000 * 3) }));

    function inEllipse(x, y, cx0, cy0, rx, ry, rot) {
      const c = Math.cos(rot), n = Math.sin(rot), dx = x - cx0, dy = y - cy0;
      const u = dx * c + dy * n, v = -dx * n + dy * c;
      return (u * u) / (rx * rx) + (v * v) / (ry * ry) <= 1;
    }
    const R = 19, RAD = Math.PI / 180;
    const seed = (() => { let v = 7; return () => (v = (v * 16807) % 2147483647) / 2147483647; })();

    /* A slight muzzle: the lower middle of the face comes forward. */
    const muzzle = (x, y, z) => Math.max(0, z / R) ** 3 * Math.exp(-(x * x) / 90 - ((y - 8) ** 2) / 70) * 5;
    function headPoint(x, y, z, kind) {
      const l = Math.hypot(x, y, z);
      return { x, y, z: z + muzzle(x, y, z), nx: x / l, ny: y / l, nz: z / l, kind, seed: seed() };
    }
    /* The mascot's face, from its SVG: drawn solid over the halftone. */
    const FACE = [
      { x: -7.5, y: -1, rx: 5.4, ry: 3.8, rot: 28 * RAD, c: 'ink' }, { x: 7.5, y: -1, rx: 5.4, ry: 3.8, rot: -28 * RAD, c: 'ink' },
      { x: 0, y: 10, rx: 2.6, ry: 1.9, rot: 0, c: 'ink' },
      { x: -7.8, y: -1.8, rx: 1.6, ry: 1.6, rot: 0, c: 'glint' }, { x: 7.8, y: -1.8, rx: 1.6, ry: 1.6, rot: 0, c: 'glint' },
    ];
    const bodyR = t => 10.5 + 7.5 * t + 2 * Math.sin(Math.PI * clamp(t * 2, 0, 1));
    function build() {
      const k = W < 760 ? .6 : 1;
      head = []; ears = []; body = []; dome = [];
      const NH = Math.round(2300 * k), golden = Math.PI * (3 - Math.sqrt(5));
      for (let i = 0; i < NH; i++) {
        const y = 1 - 2 * (i + .5) / NH, r = Math.sqrt(1 - y * y), a = i * golden;
        const px0 = Math.cos(a) * r * R, py0 = y * R, pz0 = Math.sin(a) * r * R;
        head.push(headPoint(px0, py0, pz0, pz0 > 0 && inEllipse(px0, py0, 0, 8.5, 9, 7.5, 0) ? 4 : 0));
      }
      for (const sx of [-1, 1]) {
        const NE = Math.round(210 * k), rr = 5.2;
        for (let i = 0; i < NE; i++) {
          const y = 1 - 2 * (i + .5) / NE, r = Math.sqrt(1 - y * y), a = i * golden;
          const nx = Math.cos(a) * r, nz = Math.sin(a) * r;
          ears.push({ x: sx * 18 + nx * rr, y: -6 + y * rr, z: nz * rr, nx, ny: y, nz, kind: 3, seed: seed() });
        }
      }
      const NBODY = Math.round(2700 * k);
      for (let i = 0; i < NBODY; i++) {
        const t = (i + .5) / NBODY, y = 11 + t * 52, a = i * golden;
        const r = bodyR(t), c = Math.cos(a), n = Math.sin(a);
        const x = c * r, z = n * r * .84;
        const l = Math.hypot(c, .2, n);
        const belly = n > .3 && Math.abs(x) < r * .62 && t > .14;
        body.push({ x, y, z, nx: c / l, ny: .2 / l, nz: n / l, kind: belly ? 4 : 0, seed: seed() });
      }
      const Rd = Math.min(W * .42, 372);
      for (let m = 0; m < 14; m++) for (let i = 2; i < 30; i++) dome.push({ th: m * TAU / 14, ph: i / 30 * Math.PI / 2, R: Rd });
      for (const p of [.16, .34, .52]) for (let i = 0; i < 110; i++) dome.push({ th: i * TAU / 110, ph: p * Math.PI / 2 * 1.5, R: Rd });
    }

    function measure() {
      const hr = hero.getBoundingClientRect(), qr = qa.getBoundingClientRect();
      const sizeChanged = Math.abs(hr.width - W) > 1;
      cx = qr.left - hr.left + qr.width / 2; ground = qr.top - hr.top;
      /* Nothing shows below the panel's top edge, so the canvas stops there. */
      W = hr.width; H = Math.ceil(ground + 48); dpr = Math.min(devicePixelRatio || 1, 2);
      canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
      canvas.style.height = H + 'px';
      const sh = parseFloat(getComputedStyle($('.col-mid')).paddingTop) || 200;
      s = sh / 64; headY = ground - sh + R * s + 8;
      wide = innerWidth > 1180;
      tiles = wide ? ['#t-clip', '#t-code', '#t-vault', '#t-theme'].map((q, i) => {
        const el = $(q), r = el.getBoundingClientRect(), left = i < 2;
        return { el, x: (left ? r.right : r.left) - hr.left, y: r.top - hr.top + r.height / 2, left, top: i % 2 === 0, depth: i % 2 ? 6 : 11, ph: i * 1.7 };
      }) : [];
      if (!wide) ['#t-clip', '#t-code', '#t-vault', '#t-theme'].forEach(q => { $(q).style.removeProperty('--dx'); $(q).style.removeProperty('--dy'); });
      if (sizeChanged || !head.length) build();
      if (!running) draw(performance.now(), true);
    }
    function readColors() {
      const cs = getComputedStyle(root), g = n => cs.getPropertyValue(n).trim();
      const bg = g('--bg'), fg = g('--fg'), accent = g('--accent');
      const darkPage = lum(bg) < lum(fg);
      col = {
        groups: [accent, mix(accent, darkPage ? fg : bg, .55), mix(accent, darkPage ? bg : fg, .35)],
        ink: darkPage ? mix(bg, accent, .06) : fg, glint: darkPage ? fg : bg,
        line: mix(bg, fg, .34), dome: accent, invert: !darkPage,
      };
      if (!running && head.length) draw(performance.now(), true);
    }

    const spring = (o, key, target, k, dt, damp = 1) => {
      const v = key + 'V', c = 2 * Math.sqrt(k) * damp;
      o[v] += (k * (target - o[key]) - c * o[v]) * dt; o[key] += o[v] * dt;
    };

    function plot(bucket, x, y, r) { const i = bucket.n * 3; if (i + 2 < bucket.a.length) { bucket.a[i] = x; bucket.a[i + 1] = y; bucket.a[i + 2] = r; bucket.n++; } }
    function flush(bucket, color, alpha) {
      if (!bucket.n) return;
      ctx.globalAlpha = alpha; ctx.fillStyle = color; ctx.beginPath();
      for (let i = 0, n = bucket.n * 3; i < n; i += 3) { const x = bucket.a[i], y = bucket.a[i + 1], r = bucket.a[i + 2]; ctx.moveTo(x + r, y); ctx.arc(x, y, r, 0, TAU); }
      ctx.fill(); bucket.n = 0;
    }
    function shade(nx, ny, nz) {
      const lit = Math.max(0, nx * -.42 + ny * -.6 + nz * .68), rim = (1 - Math.max(0, nz)) ** 3;
      /* Halftone: on a light page the dots are the shadow, not the light. */
      return { lam: col.invert ? 1 - lit * .9 : lit, rim };
    }

    function draw(now, still) {
      const asm = still || reduced() ? 1 : clamp((now - t0) / 1900, 0, 1);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, W, H);
      const yaw = st.yaw, pitch = st.pitch + st.nod, cy = Math.cos(yaw), sy = Math.sin(yaw), cp = Math.cos(pitch), sp = Math.sin(pitch);
      const by = yaw * .2, cby = Math.cos(by), sby = Math.sin(by);
      const baseY = headY + st.sink * (ground - headY + R * s + 26) + st.hop;
      const D = 420;

      /* The watch perimeter: a sparse dome, swept by one bright meridian. */
      const rot = now * .00006, sweep = (now * .0007) % TAU, tilt = .2, ct = Math.cos(tilt), stt = Math.sin(tilt);
      ctx.fillStyle = col.dome;
      const dl = [[], [], []];
      for (const d of dome) {
        const th = d.th + rot, x = d.R * Math.cos(d.ph) * Math.cos(th), z0 = d.R * Math.cos(d.ph) * Math.sin(th), y0 = -d.R * Math.sin(d.ph);
        const y = y0 * ct + z0 * stt, z = -y0 * stt + z0 * ct;
        let delta = Math.abs(((th - sweep) % TAU + TAU) % TAU - Math.PI) ; delta = Math.PI - delta;
        const lvl = delta < .14 ? 2 : z > 0 ? 1 : 0;
        dl[lvl].push(cx + x, ground + y);
      }
      [.07, .13, .34].forEach((a, i) => {
        ctx.globalAlpha = a * asm; ctx.beginPath();
        const arr = dl[i], r = i === 2 ? 1.25 : .95;
        for (let n = 0; n < arr.length; n += 2) { ctx.moveTo(arr[n] + r, arr[n + 1]); ctx.arc(arr[n], arr[n + 1], r, 0, TAU); }
        ctx.fill();
      });

      const assemble = (p, sxp, syp) => {
        if (asm >= 1) return [sxp, syp, 1];
        const delay = p.seed * .45 + (1 - clamp((p.y + 20) / 90, 0, 1)) * .3;
        const e = ease(clamp((asm - delay * .7) / .55, 0, 1));
        const a = p.seed * TAU;
        return [sxp + Math.cos(a) * (1 - e) * W * .28 * (.3 + p.seed), syp + (1 - e) * (ground - syp + Math.sin(a) * 40), e];
      };

      /* Body */
      const headCX = cx, headCY = baseY;
      const headRpx = (R + .5) * s;
      for (const p of body) {
        const x = p.x * cby + p.z * sby, z = -p.x * sby + p.z * cby;
        const nz = -p.nx * sby + p.nz * cby, nx = p.nx * cby + p.nz * sby;
        if (nz < -.08) continue;
        const per = D / (D - z), X = cx + x * s * per, Y = baseY + p.y * s * per;
        if (Y > ground + 34) continue;
        if (Math.hypot(X - headCX, Y - headCY) < headRpx) continue;
        const { lam, rim } = shade(nx, p.ny, nz);
        const [ax, ay, e] = assemble(p, X, Y);
        const a = clamp(.16 + lam * .9 + rim * .3, 0, .999) * e;
        const r = Math.max(.62, s * (.13 + .27 * lam) * per);
        plot(buckets[(p.kind === 4 ? NB : 0) + Math.floor(a * NB)], ax, ay, r);
      }

      const PIV = 12;
      const place = p => {
        /* Pitch then yaw, around the neck. */
        const y0 = p.y - PIV, y1 = y0 * cp + p.z * sp, z1 = -y0 * sp + p.z * cp;
        const x = p.x * cy + z1 * sy, z = -p.x * sy + z1 * cy;
        const ny1 = p.ny * cp + p.nz * sp, nz1 = -p.ny * sp + p.nz * cp;
        const nx = p.nx * cy + nz1 * sy, nz = -p.nx * sy + nz1 * cy;
        if (nz < -.06) return;
        const per = D / (D - z), X = cx + x * s * per, Y = baseY + (y1 + PIV) * s * per;
        const { lam, rim } = shade(nx, ny1, nz);
        const [ax, ay, e] = assemble(p, X, Y);
        const a = clamp(.16 + lam * .9 + rim * .34, 0, .999) * e;
        const r = Math.max(.62, s * (.13 + .27 * lam) * per);
        plot(buckets[(p.kind === 3 ? NB * 2 : p.kind === 4 ? NB : 0) + Math.floor(a * NB)], ax, ay, r);
      };
      for (const p of ears) place(p);
      for (const p of head) place(p);
      for (let gI = 0; gI < GROUPS; gI++) for (let b = 0; b < NB; b++) flush(buckets[gI * NB + b], col.groups[gI], (b + 1) / NB);
      ctx.globalAlpha = clamp((asm - .62) / .3, 0, 1);
      for (const f of FACE) {
        if (f.c === 'glint' && st.blink > .5) continue;
        const cr = Math.cos(f.rot), sr = Math.sin(f.rot);
        ctx.beginPath();
        let shown = 0;
        for (let i = 0; i < 24; i++) {
          const a = i / 24 * TAU, u = Math.cos(a) * f.rx, v = Math.sin(a) * f.ry;
          const fx = f.x + u * cr - v * sr, fy = f.y + u * sr + v * cr;
          const fz0 = Math.sqrt(Math.max(0, R * R - fx * fx - fy * fy)), fz = fz0 + muzzle(fx, fy, fz0);
          const y0 = fy - PIV, y1 = y0 * cp + fz * sp, z1 = -y0 * sp + fz * cp;
          const x = fx * cy + z1 * sy, z = -fx * sy + z1 * cy, per = D / (D - z);
          if (z > 0) shown++;
          const X = cx + x * s * per, Y = baseY + (y1 + PIV) * s * per;
          i ? ctx.lineTo(X, Y) : ctx.moveTo(X, Y);
        }
        if (shown < 12) continue;
        ctx.closePath(); ctx.fillStyle = col[f.c]; ctx.fill();
      }
      ctx.globalAlpha = 1;

      /* Leader lines from the sentry to its instruments. */
      if (tiles.length && asm > .7 && st.sink < .6) {
        ctx.strokeStyle = col.line; ctx.fillStyle = col.line; ctx.lineWidth = 1; ctx.globalAlpha = (asm - .7) / .3 * .8 * clamp(1 - st.sink / .6, 0, 1);
        const earX = 18 * s * cy, earY = baseY - 6 * s;
        for (const t of tiles) {
          const sign = t.left ? -1 : 1;
          const tx = t.x + (t.dxNow || 0), ty = t.y + (t.dyNow || 0);
          let ax, ay;
          if (t.top) { ax = cx + sign * Math.abs(earX) + sign * 6; ay = earY; }
          else { const tt = clamp(((ty - baseY) / s - 11) / 52, 0, 1); ax = cx + sign * bodyR(tt) * s; ay = ty; }
          ctx.beginPath(); ctx.moveTo(ax, ay);
          if (t.top) { const ex = ax + (tx - ax) * .42; ctx.lineTo(ex, ay); ctx.lineTo(ex + Math.sign(tx - ax) * Math.abs(ty - ay), ty); ctx.lineTo(tx, ty); }
          else ctx.lineTo(tx, ty);
          ctx.stroke();
          ctx.beginPath(); ctx.arc(ax, ay, 2.2, 0, TAU); ctx.fill();
        }
        ctx.globalAlpha = 1;
      }
    }

    function frame(now) {
      raf = 0;
      if (!running) return;
      const dt = Math.min(.05, (now - last) / 1000 || .016); last = now;
      const motion = now - pointerAt < 3200, typing = now - typingAt < 2600;
      if (typing) { tgt.yaw = 0; tgt.pitch = .4; tgt.k = 90; }
      else if (motion) {
        tgt.yaw = clamp((px - cx) / Math.max(420, W * .42), -1, 1) * .95;
        tgt.pitch = clamp((py - headY) / 520, -.4, .55); tgt.k = 80;
      } else if (now > nextSaccade) {
        /* Meerkats scan in snaps: turn fast, hold, turn again. */
        tgt.yaw = (Math.random() * 2 - 1) * .85; tgt.pitch = Math.random() * .22 - .1; tgt.k = 150;
        nextSaccade = now + 1100 + Math.random() * 2300;
      }
      spring(st, 'yaw', tgt.yaw, tgt.k, dt); spring(st, 'pitch', tgt.pitch, tgt.k, dt);
      spring(st, 'sink', tgt.sink, tgt.sink ? 60 : 110, dt, tgt.sink ? 1 : .55);
      spring(st, 'hop', 0, 170, dt, .42); spring(st, 'nod', 0, 260, dt, .5);
      if (tgt.sink) st.blink = 1;
      else if (now > nextBlink) { st.blink = 1; if (now > nextBlink + 130) { st.blink = 0; nextBlink = now + 2400 + Math.random() * 3600; } }
      if (wide) for (const t of tiles) {
        const nx = (px / Math.max(W, 1) - .5), ny = (py / Math.max(H, 1) - .5);
        t.dxNow = -nx * t.depth; t.dyNow = -ny * t.depth + Math.sin(now * .001 + t.ph) * 3.5;
        t.el.style.setProperty('--dx', t.dxNow.toFixed(2) + 'px'); t.el.style.setProperty('--dy', t.dyNow.toFixed(2) + 'px');
      }
      draw(now, false);
      raf = requestAnimationFrame(frame);
    }
    function start() {
      if (reduced()) { running = false; draw(performance.now(), true); return; }
      if (!running && visible && !document.hidden) { running = true; last = performance.now(); raf = requestAnimationFrame(frame); }
    }
    function stop() { running = false; if (raf) cancelAnimationFrame(raf); raf = 0; }

    addEventListener('pointermove', e => {
      const hr = hero.getBoundingClientRect();
      px = e.clientX - hr.left; py = e.clientY - hr.top; pointerAt = performance.now();
    }, { passive: true });
    new ResizeObserver(measure).observe(hero);
    new IntersectionObserver(es => { visible = es[0].isIntersecting; visible ? start() : stop(); }).observe(hero);
    document.addEventListener('visibilitychange', () => (document.hidden ? stop() : start()));
    matchMedia('(prefers-color-scheme: dark)').addEventListener('change', readColors);

    return {
      init() { t0 = performance.now(); readColors(); measure(); start(); },
      readColors,
      hop() { st.hopV -= 300; },
      nod() { st.nodV += 2.2; typingAt = performance.now(); },
      focusDown() { typingAt = performance.now(); },
      sink(on) { tgt.sink = on ? 1 : 0; if (reduced()) { st.sink = tgt.sink; st.blink = tgt.sink; draw(performance.now(), true); } },
    };
  })();

  /* ---------- The fictional vault, from crates/countersign/src/demo.rs ---------- */
  const WORDS = ['lantern', 'orbit', 'cactus', 'meadow', 'quill'];
  const pass = n => `${WORDS[n % 5]}-${WORDS[(n + 2) % 5]}-${n}`;
  const L = (id, name, sub, host, o = {}) => ({ id, kind: 'login', name, sub, host, ...o });
  const ITEMS = [
    L(20, 'Amazon', 'alex@fictional.dev', 'www.amazon.com'),
    L(13, 'Bank of Fiction', 'alex.morgan', 'online.bankoffiction.example', { reprompt: true }),
    L(6, 'Cloudflare', 'alex@fictional.dev', 'dash.cloudflare.com', { totp: true }),
    L(16, 'crates.io', 'alex-morgan', 'crates.io'),
    { id: 60, kind: 'ssh', name: 'Deploy key', sub: 'ED25519 key' },
    L(3, 'DigitalOcean', 'alex@fictional.dev', 'cloud.digitalocean.com', { totp: true }),
    L(19, 'Discord', 'alex#0420', 'discord.com'),
    L(17, 'Docker Hub', 'alexmorgan', 'hub.docker.com'),
    { id: 30, kind: 'card', name: 'Everyday Visa', sub: 'Visa, *4242', fav: true, number: '4111 1111 1111 4242', code: '737' },
    L(4, 'Fastmail', 'alex@fictional.dev', 'app.fastmail.com', { totp: true, fav: true }),
    L(8, 'Figma', 'alex@fictional.dev', 'www.figma.com'),
    L(2, 'Gitea', 'alex', 'git.fictional.dev'),
    L(1, 'GitHub', 'alex@fictional.dev', 'github.com', { totp: true, fav: true, passkey: true }),
    L(10, 'Hacker News', 'alexm', 'news.ycombinator.com'),
    L(5, 'Hetzner Cloud', 'alex@fictional.dev', 'console.hetzner.cloud', { totp: true }),
    L(14, 'Home router', 'admin', '192.168.1.1'),
    L(7, 'Linear', 'alex@fictional.dev', 'linear.app'),
    L(15, 'npm', 'alexm', 'www.npmjs.com', { totp: true }),
    { id: 40, kind: 'identity', name: 'Personal', sub: 'Alex Morgan' },
    L(18, 'Proton VPN', 'alex@fictional.dev', 'account.protonvpn.com'),
    L(12, 'Spotify', 'alex@fictional.dev', 'open.spotify.com'),
    L(11, 'Steam', 'alexplays', 'store.steampowered.com'),
    L(9, 'Tailscale', 'alex@fictional.dev', 'login.tailscale.com', { fav: true }),
    { id: 31, kind: 'card', name: 'Travel Mastercard', sub: 'Mastercard, *8210', number: '5555 5555 5555 8210', code: '431' },
    { id: 50, kind: 'note', name: 'Wi-Fi at home', sub: 'Network: Morgan-5G' },
  ];
  let recent = [1, 4, 30];

  /* Ranking, shaped like crates/countersign/src/search.rs. */
  function fuzzy(text, q) {
    let t = 0, score = 0, run = 0;
    const s = text.toLowerCase();
    for (const ch of q) {
      const at = s.indexOf(ch, t);
      if (at < 0) return null;
      run = at === t && t > 0 ? run + 1 : 0;
      score += 16 + run * 12 + (at === 0 || /[\s._@-]/.test(s[at - 1]) ? 10 : 0) - Math.min(8, at - t);
      t = at + 1;
    }
    return score;
  }
  function rank(query) {
    const q = query.trim().toLowerCase();
    if (!q) {
      const r = id => { const i = recent.indexOf(id); return i < 0 ? 1e9 : i; };
      return [...ITEMS].sort((a, b) => r(a.id) - r(b.id));
    }
    return ITEMS.map(it => {
      const parts = [fuzzy(it.name, q) ?? -1, it.host ? (fuzzy(it.host, q) ?? -1) : -1, it.sub ? (fuzzy(it.sub, q) ?? -1) : -1];
      const sc = [parts[0] >= 0 ? parts[0] * 3 : -1, parts[1] >= 0 ? parts[1] * 2 : -1, parts[2]];
      let best = Math.max(...sc);
      if (best < 0) return null;
      const n = it.name.toLowerCase();
      if (n.startsWith(q)) best += 400; else if (n.split(/\s+/).some(w => w.startsWith(q))) best += 200;
      if (it.host && (it.host.startsWith(q) || it.host.includes('.' + q))) best += 120;
      if (it.fav) best += 40;
      const pos = recent.indexOf(it.id);
      if (pos >= 0) best += Math.max(0, 60 - pos * 6);
      return { it, best };
    }).filter(Boolean).sort((a, b) => b.best - a.best).map(x => x.it);
  }

  const totp = (id, win) => {
    let x = (Math.imul(id, 2654435761) ^ Math.imul(win, 40503)) >>> 0;
    x = Math.imul(x ^ (x >>> 15), 2246822519) >>> 0; x = Math.imul(x ^ (x >>> 13), 3266489917) >>> 0; x = (x ^ (x >>> 16)) >>> 0;
    return String(x % 1000000).padStart(6, '0').replace(/(\d{3})(\d{3})/, '$1 $2');
  };
  const ACTION = { login: 'password', card: 'number', note: 'note', ssh: 'public key', identity: 'details' };

  /* ---------- Demo state ---------- */
  const el = {
    qa: $('#qa'), input: $('#qa-input'), count: $('#qa-count'), list: $('#qa-list'), detail: $('#qa-detail'),
    closed: $('#qa-closed'), locked: $('#qa-locked'), search: $('#qa-search'), foot: $('#qa-foot'), bar: $('#qa-bar'),
    pass: $('#qa-pass'), skip: $('#qa-skip'),
  };
  const S = {
    view: 'list', shown: [], sel: 0, clip: null, locked: false, pending: null, detail: null, reveal: false, barTimer: 0, touched: false,
    idle: ['Nothing copied', 'Press / to open quick access'],
  };
  const icon = (id, solid) => `<svg class="i${solid ? ' solid' : ''}" aria-hidden="true"><use href="#${id}"/></svg>`;
  const toneOf = name => { let h = 0; for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0; return h % 6; };

  function setView(v) {
    S.view = v;
    el.qa.dataset.view = v;
    el.list.hidden = v !== 'list'; el.detail.hidden = v !== 'detail'; el.closed.hidden = v !== 'closed';
    el.locked.hidden = v !== 'locked' && v !== 'reprompt';
    el.search.hidden = v !== 'list' && v !== 'detail';
    el.foot.hidden = v !== 'list' && v !== 'detail';
    el.bar.hidden = true;
  }
  function renderList() {
    S.shown = rank(el.input.value);
    S.sel = clamp(S.sel, 0, Math.max(0, S.shown.length - 1));
    const q = el.input.value.trim();
    el.count.textContent = q ? `${S.shown.length} ${S.shown.length === 1 ? 'match' : 'matches'}` : `${S.shown.length} items`;
    if (!S.shown.length) { el.list.innerHTML = '<li class="empty">No matches</li>'; return; }
    el.list.innerHTML = S.shown.map((it, i) => {
      const on = i === S.sel;
      const b = [
        it.passkey ? icon('i-finger') : '', it.totp ? icon('i-clock') : '',
        it.reprompt ? icon('i-lock', true) : '', it.fav ? `<span class="fav">${icon('i-star', true)}</span>` : '',
      ].join('');
      return `<li class="row" role="option" id="row-${it.id}" data-i="${i}" aria-selected="${on}">
        <span class="mono-t tone-${toneOf(it.name)}">${it.name[0].toUpperCase()}</span>
        <span class="rt"><b>${it.name}</b><small>${it.sub}</small></span>
        <span class="badges">${b}</span>${on ? `<span class="act">${icon('i-return')} ${ACTION[it.kind]}</span>` : ''}</li>`;
    }).join('');
    const cur = S.shown[S.sel];
    if (cur) el.input.setAttribute('aria-activedescendant', 'row-' + cur.id);
    updateFoot();
    tick();
  }
  function select(i) {
    if (!S.shown.length) return;
    S.sel = (i + S.shown.length) % S.shown.length;
    renderList();
    const row = el.list.querySelector('[aria-selected="true"]');
    if (row) { const t = row.offsetTop - el.list.offsetTop; if (t < el.list.scrollTop) el.list.scrollTop = t - 6; else if (t + row.offsetHeight > el.list.scrollTop + el.list.clientHeight) el.list.scrollTop = t + row.offsetHeight - el.list.clientHeight + 6; }
  }
  function updateFoot() {
    const it = S.shown[S.sel], alt = el.foot.querySelector('[data-act="alt"]');
    if (alt) alt.lastChild.textContent = it && it.kind === 'card' ? 'security code' : 'username';
  }
  function bar(msg) {
    el.bar.textContent = msg; el.bar.hidden = false;
    clearTimeout(S.barTimer); S.barTimer = setTimeout(() => { el.bar.hidden = true; }, 3800);
  }

  const FIELDS = {
    password: 'Password', username: 'Username', code: 'Verification code', number: 'Card number',
    cvv: 'Security code', note: 'Note', key: 'Public key', cardholder: 'Cardholder name',
  };
  function copy(it, field) {
    if (!it) return;
    if (it.reprompt && (field === 'password' || field === 'code') ) { S.pending = { it, field }; return openGate(true); }
    if (field === 'code' && !it.totp) return bar(`${it.name} has no verification code.`);
    S.clip = { title: `${FIELDS[field]} copied`, item: it.name, total: 30, at: Date.now(), follow: field === 'password' && it.totp ? it : null };
    recent = [it.id, ...recent.filter(r => r !== it.id)].slice(0, 6);
    sentry.hop();
    setView('closed');
    tick();
    $('#qa-reopen').focus({ preventScroll: true });
  }
  function primary(it, alt) {
    if (!it) return;
    if (alt) return it.kind === 'login' ? copy(it, 'username') : it.kind === 'card' ? copy(it, 'cvv') : primary(it, false);
    if (it.kind === 'login') copy(it, 'password');
    else if (it.kind === 'card') copy(it, 'number');
    else if (it.kind === 'note') copy(it, 'note');
    else if (it.kind === 'ssh') copy(it, 'key');
    else openDetail(it);
  }
  function openQuick() {
    if (S.locked) { setView('locked'); el.pass.focus({ preventScroll: true }); return; }
    setView('list'); el.input.value = ''; S.sel = 0; renderList(); el.input.focus({ preventScroll: true });
  }
  function closeQuick() {
    setView('closed'); tick(); $('#qa-reopen').focus({ preventScroll: true });
  }
  function openGate(reprompt) {
    $('#gate-title').textContent = reprompt ? `${S.pending.it.name} asks for your master password` : 'Unlock your vault';
    $('#gate-sub').textContent = reprompt ? 'Re-prompt is on for this item, so Countersign checks first.' : 'alex@fictional.dev · Bitwarden cloud, EU';
    $('#gate-btn').textContent = reprompt ? 'Confirm' : 'Unlock';
    el.pass.value = '';
    setView(reprompt ? 'reprompt' : 'locked');
    el.pass.focus({ preventScroll: true });
  }
  function lock() {
    if (S.locked) return;
    S.locked = true;
    if (S.clip) { S.clip = null; S.idle = ['Cleared on lock', 'Nothing left to paste']; setClipTile(...S.idle, 0); }
    S.pending = null;
    $('#vault-state').textContent = 'Locked'; $('#vault-dot').classList.add('locked'); $('#lock-btn').textContent = 'Unlock';
    sentry.sink(true); openGate(false);
  }
  function unlock() {
    S.locked = false;
    $('#vault-state').textContent = 'Unlocked'; $('#vault-dot').classList.remove('locked'); $('#lock-btn').textContent = 'Lock';
    sentry.sink(false); openQuick();
  }
  function openDetail(it) {
    if (!it) return;
    S.detail = it; S.reveal = false; renderDetail(); setView('detail');
  }
  function renderDetail() {
    const it = S.detail; if (!it) return;
    const row = (label, value, cls = '', extra = '') => `<div class="field"><div><small>${label}</small><span class="${cls}">${value}</span></div>${extra}</div>`;
    const cp = f => `<button class="icon-b" type="button" data-copy="${f}" aria-label="Copy ${FIELDS[f]}">${icon('i-copy')}</button>`;
    let fields = '';
    if (it.kind === 'login') {
      const secret = S.reveal ? pass(it.id) : '●'.repeat(12);
      fields = row('Username', it.sub, '', cp('username'))
        + row('Password', secret, S.reveal ? '' : 'secret', `<button class="icon-b" type="button" data-reveal aria-label="${S.reveal ? 'Hide' : 'Reveal'} password">${icon('i-eye')}</button>` + cp('password'))
        + (it.totp ? row('Verification code', '<b id="d-code"></b>', 'code', cp('code')) : '')
        + row('Website', it.host);
    } else if (it.kind === 'card') {
      fields = row('Cardholder', 'Alex Morgan') + row('Number', S.reveal ? it.number : '●'.repeat(12), S.reveal ? '' : 'secret', `<button class="icon-b" type="button" data-reveal aria-label="Reveal number">${icon('i-eye')}</button>` + cp('number')) + row('Security code', '●●●', 'secret', cp('cvv'));
    } else if (it.kind === 'identity') {
      fields = row('Name', 'Mx Alex Morgan') + row('Email', 'alex@fictional.dev') + row('Phone', '+27 21 555 0199') + row('Address', '12 Fictional Road, Cape Town');
    } else if (it.kind === 'note') {
      fields = row('Note', 'Network: Morgan-5G<br>Password is on the router label.<br>Guest network: Morgan-Guest', '', cp('note'));
    } else {
      fields = row('Fingerprint', 'SHA256:fICt10nAlK3yF1ngErPr1nT0nlyF0rD3m0') + row('Public key', 'ssh-ed25519 AAAAC3Nz… alex@deploy', '', cp('key'));
    }
    el.detail.innerHTML = `<div class="d-head"><span class="mono-t tone-${toneOf(it.name)}">${it.name[0].toUpperCase()}</span><div><h3>${it.name}</h3><small>${it.host || it.sub}</small></div></div><div class="fields">${fields}</div>`;
    tick();
  }
  el.detail.addEventListener('click', e => {
    const b = e.target.closest('button'); if (!b) return;
    if ('reveal' in b.dataset) { S.reveal = !S.reveal; renderDetail(); return; }
    if (b.dataset.copy) copy(S.detail, b.dataset.copy);
  });

  function act(name) {
    const it = S.view === 'detail' ? S.detail : S.shown[S.sel];
    if (name === 'copy') primary(it, false);
    else if (name === 'alt') primary(it, true);
    else if (name === 'code') it && it.kind === 'card' ? copy(it, 'cvv') : copy(it, 'code');
    else if (name === 'open') bar(it && it.host ? `Countersign would open ${it.host} in your browser and close.` : 'This item has no website.');
    else if (name === 'details') openDetail(it);
    else if (name === 'esc') esc();
  }
  function esc() {
    if (S.view === 'detail') { setView('list'); renderList(); el.input.focus({ preventScroll: true }); }
    else if (S.view === 'reprompt') { S.pending = null; setView('list'); renderList(); el.input.focus({ preventScroll: true }); }
    else if (S.view === 'list') { if (el.input.value) { el.input.value = ''; S.sel = 0; renderList(); } else closeQuick(); }
  }

  /* ---------- Instruments ---------- */
  function setClipTile(titleText, sub, frac) {
    $('#clip-title').textContent = titleText; $('#clip-sub').textContent = sub;
    $('#clip-ring').style.strokeDashoffset = 100 * (1 - frac);
    $('#clip-hist').textContent = frac > 0 ? 'history skips this' : 'empty';
  }
  function tick() {
    const now = Date.now();
    const cur = S.view === 'detail' ? S.detail : S.shown[S.sel];
    const it = cur && cur.totp ? cur : ITEMS.find(i => i.id === 1);
    const win = Math.floor(now / 30000), left = Math.ceil(30 - (now / 1000) % 30), frac = ((30 - (now / 1000) % 30) / 30);
    const val = S.locked ? '●●● ●●●' : totp(it.id, win), code = $('#code-val');
    code.textContent = val; code.classList.toggle('expiring', !S.locked && left <= 6);
    $('#code-for').textContent = it.name; $('#code-sub').textContent = S.locked ? 'vault locked' : `new code in ${left}s`;
    $('#code-ring').style.strokeDashoffset = 100 * (1 - frac);
    const d = $('#d-code');
    if (d) { d.textContent = val; d.parentElement.classList.toggle('expiring', left <= 6); }

    const c = S.clip;
    let [main, sub] = S.idle, secs = 0, fr = 0;
    if (c) {
      const elapsed = (now - c.at) / 1000;
      if (c.follow && elapsed >= 5) {
        S.clip = { title: 'Verification code copied', item: c.item, total: 30, at: now, follow: null };
        sentry.hop(); return tick();
      }
      const remain = Math.max(0, c.total - elapsed);
      if (remain <= 0) {
        S.clip = null; S.idle = ['Clipboard cleared', 'Nothing left to paste']; setClipTile(...S.idle, 0);
        return tick();
      }
      secs = Math.ceil(remain); fr = remain / c.total;
      const parts = [c.item];
      if (c.follow) parts.push(`code replaces it in ${Math.ceil(5 - elapsed)}s`);
      parts.push(`clears in ${secs}s`);
      main = c.title; sub = parts.join(' · ');
      setClipTile(main, sub, fr);
      el.skip.hidden = !c.follow;
    } else el.skip.hidden = true;
    if (S.view === 'closed') finishClosed(main, sub, secs, fr);
  }
  function finishClosed(main, sub, secs, fr) {
    if (S.view !== 'closed') return;
    $('#closed-title').textContent = main; $('#closed-sub').textContent = sub;
    $('#closed-secs').textContent = secs ? secs : '0'; $('#closed-ring').style.strokeDashoffset = 100 * (1 - fr);
  }

  /* ---------- Input ---------- */
  el.input.addEventListener('input', () => {
    S.touched = true; S.sel = 0;
    if (S.view === 'detail') setView('list');
    renderList(); sentry.nod();
  });
  el.input.addEventListener('focus', () => sentry.focusDown());
  el.input.addEventListener('keydown', e => {
    S.touched = true;
    const k = e.key, ctrl = e.ctrlKey || e.metaKey;
    const handled = () => e.preventDefault();
    if (k === 'ArrowDown') { handled(); select(S.sel + 1); }
    else if (k === 'ArrowUp') { handled(); select(S.sel - 1); }
    else if (k === 'Enter') { handled(); S.view === 'detail' ? act('copy') : primary(S.shown[S.sel], e.shiftKey); }
    else if (k === 'Escape') { handled(); esc(); }
    else if (ctrl && k.toLowerCase() === 'c' && !el.input.value.slice(el.input.selectionStart, el.input.selectionEnd)) { handled(); const it = S.shown[S.sel]; it && it.kind === 'card' ? copy(it, 'number') : copy(it, 'password'); }
    else if (ctrl && k.toLowerCase() === 'b') { handled(); const it = S.shown[S.sel]; it && it.kind === 'login' ? copy(it, 'username') : act('alt'); }
    else if (ctrl && k.toLowerCase() === 't') { handled(); act('code'); }
    else if (ctrl && k.toLowerCase() === 'u') { handled(); act('open'); }
    else if (ctrl && k.toLowerCase() === 'o') { handled(); act('details'); }
    else if (ctrl && k.toLowerCase() === 'l') { handled(); lock(); }
  });
  el.detail.addEventListener('keydown', e => { if (e.key === 'Escape') { e.preventDefault(); esc(); } });
  el.list.addEventListener('click', e => {
    const r = e.target.closest('.row'); if (!r) return;
    S.touched = true; S.sel = +r.dataset.i; primary(S.shown[S.sel], false);
  });
  el.foot.addEventListener('click', e => { const b = e.target.closest('button'); if (b) { S.touched = true; act(b.dataset.act); } });
  $('#qa-reopen').addEventListener('click', openQuick);
  el.skip.addEventListener('click', () => { if (S.clip) { S.clip.follow = null; tick(); } });
  $('#qa-gate').addEventListener('submit', e => {
    e.preventDefault();
    if (!el.pass.value) { el.pass.placeholder = 'Type anything. This vault is fictional.'; return; }
    if (S.view === 'reprompt') { const p = S.pending; S.pending = null; const it = { ...p.it, reprompt: false }; setView('list'); copy(it, p.field); }
    else unlock();
  });
  el.pass.addEventListener('keydown', e => { if (e.key === 'Escape') { e.preventDefault(); esc(); } });
  $('#lock-btn').addEventListener('click', () => { S.touched = true; S.locked ? el.pass.focus({ preventScroll: true }) : lock(); });

  const inView = n => { const r = n.getBoundingClientRect(); return r.bottom > 80 && r.top < innerHeight - 80; };
  document.addEventListener('keydown', e => {
    const t = e.target, typing = t.matches && t.matches('input,textarea');
    if (typing || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === '/') { e.preventDefault(); S.touched = true; if (!inView(el.qa)) el.qa.scrollIntoView({ block: 'center' }); openQuick(); }
    else if (t === document.body && e.key.length === 1 && S.view === 'list' && inView(el.qa)) { S.touched = true; el.input.focus({ preventScroll: true }); }
  });
  $('#try').addEventListener('pointerdown', () => { S.touched = true; });

  /* ---------- Themes ---------- */
  const grid = $('#grid-themes');
  THEMES.forEach((t, i) => {
    const b = document.createElement('button');
    b.type = 'button'; b.className = 'th'; b.dataset.i = i; b.setAttribute('aria-pressed', 'false');
    b.style.setProperty('--tbg', t.bg); b.style.setProperty('--tfg', t.fg); b.style.setProperty('--tb', t.bl);
    const name = document.createElement('span'), sw = document.createElement('div');
    name.textContent = title(t.s); sw.className = 'sw'; sw.setAttribute('aria-hidden', 'true');
    for (const c of [t.a, t.r, t.gr, t.y, t.ma]) { const d = document.createElement('i'); d.style.background = c; sw.append(d); }
    b.append(name, sw); grid.append(b);
  });
  grid.addEventListener('click', e => {
    const b = e.target.closest('.th'); if (!b) return;
    wipeTo(+b.dataset.i, e.clientX || innerWidth / 2, e.clientY || innerHeight / 2);
  });
  $('#theme-reset').addEventListener('click', e => wipeTo(-1, e.clientX || innerWidth / 2, e.clientY || innerHeight / 2));
  const ORDER = ['tokyo-night', 'flexoki-light', 'gruvbox', 'rose-pine', 'nord', 'matte-black', 'catppuccin', 'everforest', 'inkypinky', 'kanagawa'].map(s => THEMES.findIndex(t => t.s === s)).filter(i => i >= 0);
  let orderAt = -1;
  $('#theme-next').addEventListener('click', e => {
    orderAt = (orderAt + 1) % (ORDER.length + 1);
    wipeTo(orderAt === ORDER.length ? -1 : ORDER[orderAt], e.clientX || innerWidth / 2, e.clientY || innerHeight / 2);
  });

  /* ---------- The cutaway: scroll spreads the plates and walks one copy through ---------- */
  const how = $('#how'), iso = $('#iso'), packet = $('#packet'), steps = [...document.querySelectorAll('#steps li')];
  let ticking = false;
  const canScrub = () => innerWidth > 1180 && innerHeight > 620 && !reduced();
  function layoutHow() {
    how.classList.toggle('scrub', canScrub());
    iso.style.setProperty('--fit', clamp((innerHeight - 90) / 760, .68, 1).toFixed(3));
    scrub();
  }
  function scrub() {
    ticking = false;
    if (!how.classList.contains('scrub')) {
      iso.style.removeProperty('--spread');
      steps.forEach((s, i) => s.classList.toggle('on', i === steps.length - 1));
      return;
    }
    const r = how.getBoundingClientRect();
    const p = clamp(-r.top / Math.max(1, r.height - innerHeight), 0, 1);
    const spread = 34 + ease(clamp(p / .3, 0, 1)) * 92;
    iso.style.setProperty('--spread', spread.toFixed(1) + 'px');
    const seg = (a, b) => clamp((p - a) / (b - a), 0, 1);
    let z, x = 0, y = 0, secret = false;
    if (p < .4) z = 1.5 - 2 * seg(.14, .4);
    else if (p < .66) { z = -.5 + seg(.46, .66); secret = true; }
    else { z = .5; secret = true; const m = seg(.72, .92); const iw = iso.offsetWidth, ih = iso.offsetHeight; x = m * (iw * .34 - 52); y = m * (ih * .03 - 24); }
    packet.style.setProperty('--pz', (z * spread).toFixed(1) + 'px');
    packet.style.setProperty('--px', x.toFixed(1) + 'px');
    packet.style.setProperty('--py', y.toFixed(1) + 'px');
    packet.classList.toggle('secret', secret);
    const at = p < .14 ? 0 : p < .27 ? 1 : p < .44 ? 2 : p < .68 ? 3 : 4;
    steps.forEach((s, i) => s.classList.toggle('on', i === at));
  }
  function ease(t) { return 1 - (1 - t) ** 3; }
  addEventListener('scroll', () => { if (!ticking) { ticking = true; requestAnimationFrame(scrub); } }, { passive: true });
  addEventListener('resize', layoutHow);

  /* ---------- Start ---------- */
  sentry.init();
  layoutHow();
  renderList();
  setInterval(tick, 250);
  const canAutofocus = matchMedia('(hover: hover)').matches;
  if (reduced()) { el.input.value = 'git'; renderList(); }
  else {
    const q = 'git'; let i = 0;
    const step = () => {
      if (S.touched) return;
      el.input.value = q.slice(0, ++i); S.sel = 0; renderList(); sentry.nod();
      if (i < q.length) setTimeout(step, 170 + Math.random() * 110);
      else if (canAutofocus && document.activeElement === document.body) el.input.focus({ preventScroll: true });
    };
    setTimeout(step, 2100);
  }
})();
