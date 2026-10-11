// Chapter 7: one round of the real broadcast schedule, a burst of losses, and interleaving.

import { lesson } from '../engine.ts';
import { ct } from './content.ts';
import { h, hue, slider } from './ui.ts';

interface InterleaveLesson {
  groups: number;
  k: number;
  m: number;
  order: [number, number][];
  lost: boolean[];
  perGroup: { sent: number; lost: number; ok: boolean }[];
}

const state = { interleave: 1, burstStart: 6, burstLen: 6 };

export function mountBroadcast(fig: HTMLElement): void {
  const strip = h('div', { class: 'packet-strip' });
  const title = h('h4');
  const groupsBox = h('div', { class: 'group-verdicts' });
  const render = () => {
    const r = lesson<InterleaveLesson>('interleave', { shardSize: 16, k: 4, m: 2, ...state });
    title.textContent = ct('c7.round', { n: r.order.length });
    strip.replaceChildren(
      ...r.order.map(([g, i], p) =>
        h(
          'span',
          {
            class: `pk${i >= r.k ? ' parity' : ''}${r.lost[p] ? ' lost' : ''}`,
            style: `--c:${hue(g)}`,
            title: `#${p} · ${i >= r.k ? ct('c8.what.parity', { g, i }) : ct('c8.what.data', { g, i })}`,
          },
          h('span', { text: String(g) }),
        ),
      ),
    );
    groupsBox.replaceChildren(
      ...r.perGroup.map((pg, g) =>
        h(
          'div',
          { class: `gv ${pg.ok ? 'ok' : 'dead'}`, style: `--c:${hue(g)}` },
          h('b', { text: `${g}` }),
          h('span', { text: ct(pg.ok ? 'c7.survives' : 'c7.dead', { l: pg.lost, m: r.m }) }),
        ),
      ),
    );
  };
  fig.append(
    h(
      'div',
      { class: 'controls-row' },
      slider(ct('c7.interleave'), state.interleave, 1, 6, 1, (v) => ((state.interleave = v), render())),
      slider(ct('c7.burstAt'), state.burstStart, 0, 30, 1, (v) => ((state.burstStart = v), render())),
      slider(ct('c7.burstLen'), state.burstLen, 0, 12, 1, (v) => ((state.burstLen = v), render())),
    ),
    title,
    strip,
    h('h4', { text: ct('c7.groups') }),
    groupsBox,
  );
  render();
}
