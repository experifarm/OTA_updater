// Chapter 4: slicing the patch into groups and shards.

import { lesson } from '../engine.ts';
import { ct } from './content.ts';
import { h, hex2, hue, slider } from './ui.ts';

interface SliceLesson {
  patch: number[];
  shardSize: number;
  k: number;
  m: number;
  groups: { start: number; len: number; real: number; pad: number; implicit: number }[];
  packets: number;
}

const state = { shardSize: 16, k: 4, m: 2 };

export function mountSlice(fig: HTMLElement): void {
  const stats = h('p', { class: 'big-stat' });
  const groupsBox = h('div', { class: 'groups' });
  const tip = h('div', { class: 'tip mono', attrs: { role: 'status' } });

  const render = () => {
    const s = lesson<SliceLesson>('slice', state);
    const overhead = Math.round((100 * s.m * s.groups.length) / Math.max(1, s.groups.reduce((a, g) => a + g.real, 0)));
    stats.textContent = ct('c4.stats', { len: s.patch.length, groups: s.groups.length, packets: s.packets, overhead });
    groupsBox.replaceChildren(
      ...s.groups.map((g, gi) => {
        const tiles: HTMLElement[] = [];
        for (let i = 0; i < s.k; i++) {
          if (i >= g.real) {
            tiles.push(h('span', { class: 'tile implicit', title: ct('c4.implicit') }));
            continue;
          }
          const a = g.start + i * s.shardSize;
          const bytes = s.patch.slice(a, Math.min(a + s.shardSize, g.start + g.len));
          const padded = s.shardSize - bytes.length;
          const tile = h(
            'span',
            { class: 'tile data', style: `--c:${hue(gi)}` },
            padded ? h('span', { class: 'pad', style: `width:${(100 * padded) / s.shardSize}%` }) : '',
          );
          tile.addEventListener('mouseenter', () => {
            tip.textContent = `${ct('c4.data')} ${gi}.${i} · ${bytes.map(hex2).join(' ')}${padded ? ` + ${padded}× 00 (${ct('c4.pad')})` : ''}`;
          });
          tiles.push(tile);
        }
        for (let i = 0; i < s.m; i++) {
          const tile = h('span', { class: 'tile parity', style: `--c:${hue(gi)}` });
          tile.addEventListener('mouseenter', () => (tip.textContent = `${ct('c4.parity')} ${gi}.${s.k + i} · ${ct('c4.parityTip')}`));
          tiles.push(tile);
        }
        return h(
          'div',
          { class: 'group-card' },
          h('span', { class: 'gc-title', text: ct('c4.group', { g: gi, a: g.start, b: g.start + g.len - 1 }) }),
          h('div', { class: 'tiles' }, ...tiles),
        );
      }),
    );
  };

  fig.append(
    h(
      'div',
      { class: 'controls-row' },
      slider(ct('c4.shard'), state.shardSize, 8, 64, 8, (v) => ((state.shardSize = v), render()), (v) => `${v} B`),
      slider(ct('c4.k'), state.k, 1, 8, 1, (v) => ((state.k = v), render())),
      slider(ct('c4.m'), state.m, 0, 4, 1, (v) => ((state.m = v), render())),
    ),
    stats,
    h(
      'p',
      { class: 'legend-inline' },
      h('span', { class: 'tile data inline', style: `--c:${hue(1)}` }),
      ct('c4.data'),
      h('span', { class: 'tile parity inline', style: `--c:${hue(1)}` }),
      ct('c4.parity'),
      h('span', { class: 'tile data inline', style: `--c:${hue(1)}` }, h('span', { class: 'pad', style: 'width:50%' })),
      ct('c4.pad'),
      h('span', { class: 'tile implicit inline' }),
      ct('c4.implicit'),
    ),
    groupsBox,
    tip,
  );
  render();
}
