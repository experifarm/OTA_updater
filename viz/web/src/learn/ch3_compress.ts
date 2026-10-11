// Chapter 3: why the diff stream compresses so well.

import { lesson } from '../engine.ts';
import { fmtBytes } from '../i18n.ts';
import { ct } from './content.ts';
import { h, hex2 } from './ui.ts';

interface CompressLesson {
  newSize: number;
  rawLen: number;
  zeros: number;
  single: number;
  framed: number;
  extraBytes: number;
  map: number[];
  cellBytes: number;
  hist: number[];
  sample: number[];
  sampleAt: number;
}

const cache = new Map<string, CompressLesson>();
const state = { scale: 'real' };

function get(scale: string): CompressLesson {
  let c = cache.get(scale);
  if (!c) {
    c = lesson<CompressLesson>('compress', { scale });
    cache.set(scale, c);
  }
  return c;
}

export function mountCompress(fig: HTMLElement): void {
  const body = h('div');
  const select = h(
    'select',
    { on: { change: (e) => ((state.scale = (e.target as HTMLSelectElement).value), render()) } },
    h('option', { text: 'Real · 1 MiB', attrs: { value: 'real', selected: state.scale === 'real' } }),
    h('option', { text: 'Toy · 288 B', attrs: { value: 'toy', selected: state.scale === 'toy' } }),
  );
  const render = () => {
    const c = get(state.scale);
    const side = Math.ceil(Math.sqrt(c.map.length));
    const canvas = h('canvas', { class: 'bytemap', attrs: { width: side, height: side } });
    const ctx = canvas.getContext('2d');
    if (ctx) {
      const img = ctx.createImageData(side, side);
      c.map.forEach((v, i) => {
        // dark = all zero; brighter = more non-zero bytes in the cell
        const a = v === 0 ? 0 : 0.35 + (0.65 * v) / 255;
        img.data[i * 4] = Math.round(30 + a * 190);
        img.data[i * 4 + 1] = Math.round(25 + a * 120);
        img.data[i * 4 + 2] = Math.round(48 + a * 207);
        img.data[i * 4 + 3] = 255;
      });
      ctx.putImageData(img, 0, 0);
    }
    const max = Math.max(c.newSize, c.rawLen);
    const bar = (label: string, n: number, cls: string) =>
      h(
        'div',
        { class: 'sizebar' },
        h('span', { class: 'sb-label', text: label }),
        h('span', { class: 'sb-track' }, h('span', { class: `sb-fill ${cls}`, style: `width:${Math.max(0.6, (100 * n) / max)}%` })),
        h('span', { class: 'sb-num mono', text: `${fmtBytes(n)} · ${((100 * n) / c.newSize).toFixed(1)}%` }),
      );
    const top = c.hist
      .map((count, value) => ({ value, count }))
      .sort((a, b) => b.count - a.count)
      .slice(0, 8);
    body.replaceChildren(
      h(
        'div',
        { class: 'two-col' },
        h(
          'div',
          null,
          h('h4', { text: ct('c3.map', { cell: c.cellBytes }) }),
          canvas,
          h('p', { class: 'big-stat', text: ct('c3.zeros', { pct: ((100 * c.zeros) / c.rawLen).toFixed(1) }) }),
        ),
        h(
          'div',
          null,
          h('h4', { text: ct('c3.sizes') }),
          bar(ct('c3.newImage'), c.newSize, 'new'),
          bar(ct('c3.raw'), c.rawLen, 'raw'),
          bar(ct('c3.single'), c.single, 'z1'),
          bar(ct('c3.framed'), c.framed, 'zf'),
          h('h4', { text: ct('c3.hist') }),
          h(
            'div',
            { class: 'hist' },
            ...top.map((x) =>
              h(
                'div',
                { class: 'hist-row' },
                h('span', { class: 'mono', text: `0x${hex2(x.value)}` }),
                h('span', { class: 'sb-track' }, h('span', { class: 'sb-fill raw', style: `width:${(100 * x.count) / top[0].count}%` })),
                h('span', { class: 'mono muted', text: `${((100 * x.count) / c.rawLen).toFixed(2)}%` }),
              ),
            ),
          ),
        ),
      ),
      h('h4', { text: `${ct('c3.sample')} @${c.sampleAt}` }),
      h('div', { class: 'chips' }, ...c.sample.map((b) => h('span', { class: `chip diff${b === 0 ? ' zero' : ''}`, text: hex2(b) }))),
    );
  };
  fig.append(h('div', { class: 'inline-fields' }, h('label', null, h('span', { text: ct('c3.scale') }), select)), body);
  render();
}
