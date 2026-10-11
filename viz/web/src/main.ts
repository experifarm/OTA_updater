// App controller: routes between the guided chapters and the broadcast sandbox, runs the real engine
// (WASM), drives sandbox playback and keeps the DOM panels in sync.

import { engineVersion, initEngine, scenario } from './engine.ts';
import { deviceName, fmtBytes, getLang, setLang, t, type Lang } from './i18n.ts';
import { ct } from './learn/content.ts';
import { leaveChapter, renderChapter } from './learn/index.ts';
import { RxState, indexDeliveries } from './model.ts';
import {
  PRESETS,
  buildConfig,
  type PresetId,
  type ReceiverSpec,
  type Scale,
  type ScenarioConfig,
} from './presets.ts';
import { C, Renderer, groupColor } from './render.ts';
import { buildMoments, type Moment } from './story.ts';
import type { Delivery, ScenarioLog } from './types.ts';

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

const TAIL_MS = 1600; // keep playing a little after the last event so final effects fade out

interface Overrides {
  global: Partial<ScenarioConfig>;
  receivers: Partial<ReceiverSpec>[];
}

const state = {
  preset: 'wifi' as PresetId,
  scale: 'toy' as Scale,
  speed: 1,
  playing: false,
  T: 0,
  log: null as ScenarioLog | null,
  rxs: [] as RxState[],
  byPacket: [] as Delivery[][][],
  moments: [] as Moment[],
  overrides: null as Overrides | null,
  shownMoments: -1,
  shownWritten: -1,
  view: 'learn' as 'learn' | 'sandbox',
  chapter: 1,
};

const canvas = $<HTMLCanvasElement>('stage');
const renderer = new Renderer(canvas);

// ------------------------------------------------------------------ engine

function currentConfig(): ScenarioConfig {
  const cfg = buildConfig(state.preset, state.scale, state.overrides?.global ?? {});
  if (state.overrides) cfg.receivers = cfg.receivers.map((r, i) => ({ ...r, ...state.overrides!.receivers[i] }));
  return cfg;
}

function run(): void {
  const busy = $('busy');
  const err = $('error');
  busy.hidden = false;
  err.hidden = true;
  state.playing = false;
  // Let the overlay paint before the (synchronous) simulation runs.
  setTimeout(() => {
    try {
      const log = scenario(currentConfig());
      state.log = log;
      state.rxs = log.receivers.map((rx) => new RxState(log, rx));
      state.byPacket = indexDeliveries(log);
      state.moments = buildMoments(log);
      state.T = 0;
      state.shownMoments = -1;
      state.shownWritten = -1;
      renderStatic();
      state.playing = !window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    } catch (e) {
      err.textContent = String(e instanceof Error ? e.message : e);
      err.hidden = false;
    } finally {
      busy.hidden = true;
    }
  }, 30);
}

// ------------------------------------------------------------------ static panels

function applyI18n(): void {
  document.querySelectorAll<HTMLElement>('[data-i18n]').forEach((el) => {
    el.textContent = t(el.dataset.i18n!);
  });
  document.title = t('title');
  $('prev').title = t('prev');
  $('next').title = t('next');
  $('restart').title = t('restart');
  $('prev').setAttribute('aria-label', t('prev'));
  $('next').setAttribute('aria-label', t('next'));
  $('restart').setAttribute('aria-label', t('restart'));
  document.querySelectorAll<HTMLButtonElement>('[data-lang]').forEach((b) => {
    b.setAttribute('aria-pressed', String(b.dataset.lang === getLang()));
  });
  updatePlayButton();
}

function buildPresetButtons(): void {
  const box = $('presets');
  box.replaceChildren();
  for (const p of PRESETS) {
    const b = document.createElement('button');
    b.type = 'button';
    b.setAttribute('role', 'radio');
    b.setAttribute('aria-checked', String(p.id === state.preset));
    b.textContent = t(`preset.${p.id}`);
    b.title = t(`preset.${p.id}.d`);
    b.addEventListener('click', () => {
      state.preset = p.id;
      state.overrides = null;
      history.replaceState(null, '', `#sandbox/${p.id}`);
      buildPresetButtons();
      run();
    });
    box.append(b);
  }
  $('preset-desc').textContent = t(`preset.${state.preset}.d`);
}

function renderStatic(): void {
  const log = state.log;
  if (!log) return;
  applyI18n();
  buildPresetButtons();
  ($('scale') as HTMLSelectElement).value = state.scale;

  // The update card: what the signed manifest says.
  const m = log.manifest;
  const s = log.summary;
  const rows: [string, string][] = [
    [t('card.session'), m.session],
    [t('card.fw'), `v1.0.0 → v1.1.0 (#${m.fwVersion})`],
    [t('band.patch'), `${fmtBytes(s.patchSize)} (${((100 * s.patchSize) / s.newSize).toFixed(1)}%)${s.compressed ? ' zstd' : ''}`],
    [t('card.newsha'), m.newSha],
    [t('card.sig'), m.signature],
    [t('card.key'), m.publicKey],
  ];
  const card = $('card');
  card.replaceChildren();
  for (const [k, v] of rows) {
    const dt = document.createElement('dt');
    dt.textContent = k;
    const dd = document.createElement('dd');
    dd.textContent = v;
    dd.title = v;
    card.append(dt, dd);
  }

  // Legend.
  const legend = $('legend');
  legend.replaceChildren();
  const items: [string, string][] = [
    ['leg.data', groupColor(1)],
    ['leg.parity', `repeating-linear-gradient(135deg, ${groupColor(1, 1, 46)} 0 3px, #0b1020 3px 5px)`],
    ['leg.pad', 'transparent; outline: 1px dashed #26324f'],
    ['leg.manifest', C.gold],
    ['leg.attacker', C.attacker],
    ['leg.lost', `linear-gradient(45deg, transparent 42%, ${C.bad} 42% 58%, transparent 58%)`],
    ['leg.corrupt', `transparent; outline: 2px solid ${C.bad}`],
    ['leg.rebuilt', `${groupColor(4)}; outline: 1px solid #fff`],
    ['leg.diff', C.neu],
    ['leg.extra', C.extra],
  ];
  for (const [key, bg] of items) {
    const li = document.createElement('li');
    const i = document.createElement('i');
    i.setAttribute('style', `background: ${bg}`);
    li.append(i, t(key));
    legend.append(li);
  }

  buildCustomize();
  renderToy();
  drawMarks();
  $('foot').textContent = `${engineVersion()} · ${s.packetsPerRound} packets/round · ${s.groups} groups · deterministic (seed ${currentConfig().seed})`;
}

function buildCustomize(): void {
  const body = $('custom-body');
  body.replaceChildren();
  const cfg = currentConfig();
  const draft: Overrides = {
    global: { ...(state.overrides?.global ?? {}) },
    receivers: cfg.receivers.map((_, i) => ({ ...(state.overrides?.receivers[i] ?? {}) })),
  };
  const slider = (
    label: string,
    value: number,
    min: number,
    max: number,
    step: number,
    fmt: (v: number) => string,
    onChange: (v: number) => void,
  ) => {
    const l = document.createElement('label');
    const span = document.createElement('span');
    span.textContent = label;
    const input = document.createElement('input');
    input.type = 'range';
    input.min = String(min);
    input.max = String(max);
    input.step = String(step);
    input.value = String(value);
    const out = document.createElement('output');
    out.textContent = fmt(value);
    input.addEventListener('input', () => {
      const v = Number(input.value);
      out.textContent = fmt(v);
      onChange(v);
    });
    l.append(span, input, out);
    return l;
  };
  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const int = (v: number) => String(v);
  const g = document.createElement('div');
  g.append(
    slider('k (data)', cfg.k, 1, 16, 1, int, (v) => (draft.global.k = v)),
    slider('m (parity)', cfg.m, 0, 8, 1, int, (v) => (draft.global.m = v)),
    slider('interleave', cfg.interleave, 1, 8, 1, int, (v) => (draft.global.interleave = v)),
    slider(t('seed'), cfg.seed, 1, 50, 1, int, (v) => (draft.global.seed = v)),
  );
  body.append(g);
  cfg.receivers.forEach((r, i) => {
    const box = document.createElement('div');
    box.className = 'rx';
    const name = document.createElement('b');
    name.textContent = deviceName(r.name);
    box.append(
      name,
      slider('loss', r.loss ?? 0, 0, 0.6, 0.01, pct, (v) => (draft.receivers[i].loss = v)),
      slider('burst ×', r.burstLen ?? 1, 1, 10, 0.5, (v) => v.toFixed(1), (v) => (draft.receivers[i].burstLen = v)),
      slider('corrupt', r.corrupt ?? 0, 0, 0.2, 0.01, pct, (v) => (draft.receivers[i].corrupt = v)),
      slider('dup', r.dup ?? 0, 0, 0.2, 0.01, pct, (v) => (draft.receivers[i].dup = v)),
      slider('reorder', r.reorder ?? 0, 0, 0.3, 0.01, pct, (v) => (draft.receivers[i].reorder = v)),
    );
    body.append(box);
  });
  const row = document.createElement('div');
  row.className = 'row';
  const go = document.createElement('button');
  go.type = 'button';
  go.textContent = t('rerun');
  go.addEventListener('click', () => {
    state.overrides = draft;
    run();
  });
  row.append(go);
  body.append(row);
}

function renderToy(): void {
  const box = $('toy');
  const log = state.log;
  if (!log?.toy) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  box.replaceChildren();
  const fig = (caption: string, pre: HTMLPreElement, id?: string) => {
    const f = document.createElement('figure');
    const c = document.createElement('figcaption');
    c.textContent = caption;
    if (id) pre.id = id;
    f.append(c, pre);
    return f;
  };
  const oldPre = document.createElement('pre');
  oldPre.textContent = log.toy.old;
  const newPre = document.createElement('pre');
  appendClassified(newPre, log.toy.new, log.toy.class, log.toy.new.length);
  box.append(
    fig(t('band.old'), oldPre),
    fig(`${t('band.new')} — ${t('band.diff')} / ${t('band.extra')}`, newPre),
    fig(`${deviceName(log.receivers[0].spec.name)} · ${t('rx.write')}`, document.createElement('pre'), 'slotb'),
  );
  state.shownWritten = -1;
}

/** Text with runs wrapped in spans by byte class (0 same, 1 changed, 2 extra); bytes ≥ `upto` dimmed. */
function appendClassified(pre: HTMLElement, text: string, cls: number[], upto: number): void {
  const bytes = new TextEncoder().encode(text);
  const dec = new TextDecoder();
  let start = 0;
  const kind = (i: number) => (i >= upto ? 3 : cls[i] ?? 0);
  for (let i = 1; i <= bytes.length; i++) {
    if (i < bytes.length && kind(i) === kind(start)) continue;
    const piece = dec.decode(bytes.subarray(start, i));
    const k = kind(start);
    if (k === 0) pre.append(piece);
    else {
      const span = document.createElement('span');
      span.className = k === 1 ? 'chg' : k === 2 ? 'ext' : 'pending';
      span.textContent = k === 3 ? piece.replace(/[^\n]/g, '·') : piece;
      pre.append(span);
    }
    start = i;
  }
}

function drawMarks(): void {
  const c = $<HTMLCanvasElement>('marks');
  const log = state.log;
  const dpr = window.devicePixelRatio || 1;
  const rect = c.getBoundingClientRect();
  c.width = Math.max(1, rect.width * dpr);
  c.height = Math.max(1, rect.height * dpr);
  const ctx = c.getContext('2d');
  if (!ctx || !log) return;
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, rect.width, rect.height);
  const end = log.endMs + TAIL_MS;
  const x = (tm: number) => 8 + (tm / end) * (rect.width - 16);
  ctx.fillStyle = '#3a4566';
  for (const r of log.rounds) ctx.fillRect(x(r), 0, 2, rect.height);
  const tone: Record<string, string> = { info: '#4c8dff', good: C.good, warn: C.warn, bad: C.bad };
  for (const m of state.moments) {
    if (m.t < 1) continue;
    ctx.fillStyle = tone[m.tone];
    ctx.fillRect(x(m.t) - 1, 3, 3, rect.height - 6);
  }
}

// ------------------------------------------------------------------ dynamic panels

function updateFeed(): void {
  const visible = state.moments.filter((m) => m.t <= state.T);
  if (visible.length === state.shownMoments) return;
  state.shownMoments = visible.length;
  const feed = $('feed');
  feed.replaceChildren();
  for (const m of visible.slice(-7).reverse()) {
    const li = document.createElement('li');
    li.className = m.tone;
    const when = document.createElement('span');
    when.className = 'when';
    when.textContent = `${(m.t / 1000).toFixed(1)}s`;
    li.append(when, m.text());
    feed.append(li);
  }
}

function updateToySlot(): void {
  const log = state.log;
  const pre = document.getElementById('slotb');
  if (!log?.toy || !pre) return;
  const written = state.rxs[0]?.written ?? 0;
  if (written === state.shownWritten) return;
  state.shownWritten = written;
  pre.replaceChildren();
  appendClassified(pre, log.toy.new, log.toy.class, written);
}

function updatePlayButton(): void {
  const b = $('play');
  b.textContent = state.playing ? '❚❚' : '▶';
  b.title = state.playing ? t('pause') : t('play');
  b.setAttribute('aria-label', b.title);
}

function endTime(): number {
  return (state.log?.endMs ?? 0) + TAIL_MS;
}

function seek(T: number): void {
  state.T = Math.max(0, Math.min(endTime(), T));
}

// ------------------------------------------------------------------ loop

let last = performance.now();
let wasPlaying = state.playing;

function frame(now: number): void {
  const dt = Math.min(100, now - last);
  last = now;
  if (state.log && state.view === 'sandbox') {
    if (state.playing) {
      state.T += dt * state.speed;
      if (state.T >= endTime()) {
        state.T = endTime();
        state.playing = false;
      }
    }
    for (const rx of state.rxs) rx.advance(state.T);
    renderer.draw(state.log, state.rxs, state.byPacket, state.T);
    updateFeed();
    updateToySlot();
    const scrub = $<HTMLInputElement>('scrub');
    if (document.activeElement !== scrub) scrub.value = String(Math.round((1000 * state.T) / endTime()));
    $('clock').textContent = `${(Math.min(state.T, state.log.endMs) / 1000).toFixed(1)} s / ${(state.log.endMs / 1000).toFixed(1)} s`;
  }
  if (wasPlaying !== state.playing) {
    wasPlaying = state.playing;
    updatePlayButton();
  }
  requestAnimationFrame(frame);
}

function jump(direction: 1 | -1): void {
  const times = state.moments.map((m) => m.t).filter((x) => x >= 1);
  if (direction > 0) {
    const nxt = times.find((x) => x > state.T + 30);
    seek(nxt !== undefined ? nxt + 60 : endTime());
  } else {
    const prev = [...times].reverse().find((x) => x < state.T - 400);
    seek(prev !== undefined ? prev + 60 : 0);
  }
}

// ------------------------------------------------------------------ wiring

function wire(): void {
  $('play').addEventListener('click', () => {
    if (!state.playing && state.T >= endTime()) state.T = 0;
    state.playing = !state.playing;
  });
  $('restart').addEventListener('click', () => {
    state.T = 0;
    state.playing = true;
  });
  $('prev').addEventListener('click', () => jump(-1));
  $('next').addEventListener('click', () => jump(1));
  $<HTMLInputElement>('scrub').addEventListener('input', (e) => {
    seek((Number((e.target as HTMLInputElement).value) / 1000) * endTime());
  });
  $<HTMLSelectElement>('speed').addEventListener('change', (e) => {
    state.speed = Number((e.target as HTMLSelectElement).value);
  });
  $<HTMLSelectElement>('scale').addEventListener('change', (e) => {
    state.scale = (e.target as HTMLSelectElement).value as Scale;
    state.overrides = null;
    run();
  });
  document.querySelectorAll<HTMLButtonElement>('[data-lang]').forEach((b) =>
    b.addEventListener('click', () => {
      setLang(b.dataset.lang as Lang);
      state.shownMoments = -1;
      applyI18n();
      applyModeLabels();
      if (state.view === 'sandbox') renderStatic();
      else renderChapter($('learn-view'), state.chapter);
    }),
  );
  window.addEventListener('keydown', (e) => {
    if (state.view !== 'sandbox') return;
    const tag = (e.target as HTMLElement).tagName;
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA' || tag === 'BUTTON') return;
    if (e.key === ' ') {
      e.preventDefault();
      $('play').click();
    } else if (e.key === 'ArrowRight') jump(1);
    else if (e.key === 'ArrowLeft') jump(-1);
    else if (e.key === 'Home') seek(0);
  });
  new ResizeObserver(() => {
    renderer.resize();
    drawMarks();
  }).observe(canvas);
  window.addEventListener('hashchange', route);
}

function applyModeLabels(): void {
  const learn = $('mode-learn');
  const sandbox = $('mode-sandbox');
  learn.textContent = ct('nav.learn');
  sandbox.textContent = ct('nav.sandbox');
  learn.setAttribute('href', `#learn/${state.chapter}`);
  learn.toggleAttribute('aria-current', state.view === 'learn');
  sandbox.toggleAttribute('aria-current', state.view === 'sandbox');
}

/** `#learn/N` → chapter N; `#sandbox` or `#sandbox/<preset>` → the sandbox (optionally on a preset). */
function route(): void {
  const hash = location.hash || '#learn/1';
  const sandbox = /^#sandbox(?:\/(\w+))?$/.exec(hash);
  if (sandbox) {
    leaveChapter();
    state.view = 'sandbox';
    $('learn-view').hidden = true;
    $('sandbox-view').hidden = false;
    renderer.resize();
    const preset = PRESETS.find((p) => p.id === sandbox[1]);
    if (preset && preset.id !== state.preset) {
      state.preset = preset.id;
      state.overrides = null;
      buildPresetButtons();
      run();
    } else if (!state.log) run();
    else {
      renderStatic();
      state.shownMoments = -1;
    }
  } else {
    state.view = 'learn';
    state.playing = false;
    state.chapter = Number(/^#learn\/(\d+)/.exec(hash)?.[1] ?? 1);
    $('sandbox-view').hidden = true;
    $('learn-view').hidden = false;
    $('foot').textContent = engineVersion();
    renderChapter($('learn-view'), state.chapter);
    window.scrollTo({ top: 0 });
  }
  applyModeLabels();
}

async function boot(): Promise<void> {
  setLang(getLang());
  applyI18n();
  buildPresetButtons();
  wire();
  // Read-only hook for automated browser tests.
  (window as unknown as { __ota: unknown }).__ota = {
    ready: () => state.log !== null && getComputedStyle($('busy')).display === 'none',
    time: () => ({ T: state.T, end: endTime() }),
    statuses: () => state.rxs.map((r) => r.status),
    seek: (T: number) => seek(T),
    view: () => ({ view: state.view, chapter: state.chapter }),
  };
  try {
    await initEngine();
  } catch (e) {
    const err = $('error');
    err.textContent = `WebAssembly failed to load: ${e instanceof Error ? e.message : e}`;
    err.hidden = false;
    $('sandbox-view').hidden = false;
    return;
  }
  $('foot').textContent = engineVersion();
  route();
  requestAnimationFrame(frame);
}

void boot();
