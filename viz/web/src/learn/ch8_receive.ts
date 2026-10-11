// Chapter 8: the CRC check on a real packet, and stepping through a real device receiving a broadcast.

import { lesson, scenario } from '../engine.ts';
import { buildConfig } from '../presets.ts';
import type { Delivery, RxEvent, ScenarioLog } from '../types.ts';
import { ct } from './content.ts';
import { button, h, hex2, hue } from './ui.ts';

interface CrcLesson {
  bytes: number[];
  original: number[];
  fields: { name: string; start: number; end: number }[];
  stored: number;
  computed: number;
  accepted: boolean;
  error: string | null;
}

const state = { bit: -1 };
const hex8 = (v: number) => (v >>> 0).toString(16).padStart(8, '0');

export function mountReceive(fig: HTMLElement): () => void {
  fig.append(crcPanel(), h('h4', { text: ct('c8.stepper') }));
  const stop = stepper(fig);
  return stop;
}

function crcPanel(): HTMLElement {
  const grid = h('div', { class: 'packet-bytes' });
  const verdict = h('p', { class: 'verdict' });
  const legend = h('p', { class: 'legend-inline' });
  const render = () => {
    const r = lesson<CrcLesson>('crc', { bit: state.bit });
    const fieldOf = (i: number) => r.fields.findIndex((f) => i >= f.start && i < f.end);
    legend.replaceChildren(...r.fields.map((f, k) => h('span', { class: `chip f${k}`, text: f.name })));
    grid.replaceChildren(
      ...r.bytes.map((b, i) =>
        h('button', {
          class: `chip f${fieldOf(i)}${b !== r.original[i] ? ' flipped' : ''}`,
          text: hex2(b),
          title: `#${i} · ${r.fields[fieldOf(i)]?.name}`,
          attrs: { type: 'button' },
          on: {
            click: () => {
              state.bit = state.bit === i * 8 ? -1 : i * 8;
              render();
            },
          },
        }),
      ),
    );
    verdict.className = `verdict ${r.accepted ? 'good' : 'bad'}`;
    verdict.textContent = r.accepted
      ? ct('c8.crcOk', { c: hex8(r.computed) })
      : ct('c8.crcBad', { s: hex8(r.stored), c: hex8(r.computed) });
  };
  render();
  return h(
    'section',
    { class: 'sub' },
    h('h4', { text: ct('c8.packet') }),
    legend,
    grid,
    h('div', { class: 'row-actions' }, button(ct('c8.restore'), () => ((state.bit = -1), render()))),
    verdict,
  );
}

interface Step {
  d: Delivery;
  events: RxEvent[];
}

let cachedLog: ScenarioLog | null = null;

function stepper(fig: HTMLElement): () => void {
  cachedLog ??= scenario(
    buildConfig('clean', 'toy', {
      maxRounds: 4,
      receivers: [{ name: 'Car', loss: 0.25, corrupt: 0.06, seed: 21 }],
    }),
  );
  const log = cachedLog;
  const rx = log.receivers[0];
  const steps: Step[] = rx.deliveries.map((d) => ({ d, events: [] }));
  rx.events.forEach((e, i) => steps[rx.eventDelivery[i]]?.events.push(e));
  const s = log.summary;
  const n = s.k + s.m;

  const desc = h('p', { class: 'step-desc' });
  const evList = h('ul', { class: 'step-events' });
  const grid = h('div', { class: 'rx-grid', style: `--cols:${s.groups}` });
  const progress = h('p', { class: 'muted' });
  const counter = h('span', { class: 'mono muted' });
  let pos = 0;
  let timer: ReturnType<typeof setInterval> | undefined;

  const render = () => {
    // Replay events of steps [0, pos) into a fresh grid state.
    const cells = new Array<number>(s.groups * n).fill(0);
    const done = new Array<boolean>(s.groups).fill(false);
    const fed = new Array<boolean>(s.groups).fill(false);
    let written = 0;
    const apply = (e: RxEvent) => {
      switch (e.type) {
        case 'manifest_ok':
          for (let x = 0; x < cells.length; x++) if (cells[x] === 1) cells[x] = 0;
          log.groups.forEach((g, gi) => {
            for (let i = g.real; i < s.k; i++) cells[gi * n + i] = 4;
          });
          break;
        case 'buffered':
          cells[e.g * n + e.i] = 1;
          break;
        case 'stored':
          cells[e.g * n + e.i] = 2;
          break;
        case 'decoded':
          if (e.ok) {
            for (const i of e.missing) cells[e.g * n + i] = 3;
            done[e.g] = true;
          } else for (let i = 0; i < n; i++) if (cells[e.g * n + i] !== 4) cells[e.g * n + i] = 0;
          break;
        case 'released':
          fed[e.g] = true;
          break;
        case 'write':
          written = Math.max(written, e.off + e.len);
          break;
        default:
          break;
      }
    };
    for (let k = 0; k < pos; k++) steps[k].events.forEach(apply);

    const cur = steps[pos - 1];
    if (cur) {
      const pk = log.packets[cur.d.p];
      const what =
        pk.kind === 0
          ? ct('c8.what.manifest')
          : ct(pk.i < s.k ? 'c8.what.data' : 'c8.what.parity', { g: pk.g, i: pk.i });
      desc.replaceChildren(
        h('span', { class: `fate f-${cur.d.f}`, text: ct(`c8.f.${cur.d.f}`) }),
        ' ',
        ct('c8.pkt', { p: cur.d.p, what }),
      );
      evList.replaceChildren(...cur.events.map((e) => h('li', { class: `ev-${e.type}`, text: eventText(e, s.k) })));
    } else {
      desc.textContent = '';
      evList.replaceChildren();
    }
    grid.replaceChildren(
      ...Array.from({ length: s.groups }, (_, g) =>
        h(
          'div',
          { class: `rx-col${done[g] ? ' done' : ''}${fed[g] ? ' fed' : ''}`, style: `--c:${hue(g)}` },
          h('span', { class: 'rx-g', text: String(g) }),
          ...Array.from({ length: n }, (_, i) =>
            h('span', { class: `rc c${cells[g * n + i]}${i >= s.k ? ' par' : ''}${i === s.k ? ' first-par' : ''}` }),
          ),
        ),
      ),
    );
    progress.textContent = ct('c8.progress', {
      done: done.filter(Boolean).length,
      total: s.groups,
      w: written,
      n: s.newSize,
    });
    counter.textContent = `${pos}/${steps.length}`;
  };

  const step = () => {
    if (pos < steps.length) pos++;
    else stopTimer();
    render();
  };
  const stopTimer = () => {
    if (timer) clearInterval(timer);
    timer = undefined;
    playBtn.textContent = ct('c8.play');
  };
  const playBtn = button(ct('c8.play'), () => {
    if (timer) return stopTimer();
    if (pos >= steps.length) pos = 0;
    timer = setInterval(step, 420);
    playBtn.textContent = ct('c8.pause');
  });
  fig.append(
    h(
      'div',
      { class: 'row-actions' },
      button(ct('c8.step'), step, 'primary'),
      playBtn,
      button(ct('c8.reset'), () => {
        stopTimer();
        pos = 0;
        render();
      }),
      counter,
    ),
    h('div', { class: 'two-col wide-left' }, h('div', null, grid, progress), h('div', null, desc, evList)),
  );
  render();
  return stopTimer;
}

function eventText(e: RxEvent, k: number): string {
  switch (e.type) {
    case 'decoded':
      return e.ok
        ? ct('c8.e.decoded', { g: e.g, k, missing: e.missing.length ? ct('c8.e.missing', { n: e.missing.length }) : '' })
        : ct('c8.e.hashbad', { g: e.g });
    case 'released':
      return ct('c8.e.released', { g: e.g });
    case 'write':
      return ct('c8.e.write', { n: e.len });
    case 'failed':
      return ct('c8.e.failed', { r: e.reason });
    case 'verified':
      return ct(e.ok ? 'c8.e.verified' : 'c8.e.verifyBad');
    case 'manifest_frag':
      return ct('c8.e.manifest_frag', { i: e.index + 1, n: e.count });
    case 'rejected':
      return ct('c8.e.failed', { r: e.reason });
    default:
      return ct(`c8.e.${e.type}`);
  }
}
