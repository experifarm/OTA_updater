// Chapter 5: Reed-Solomon — lose shards, watch the real decoder rebuild them, and see one byte computed.

import { lesson } from '../engine.ts';
import { ct } from './content.ts';
import { bits8, debounce, glyph, h, hex2, slider } from './ui.ts';

interface RsLesson {
  k: number;
  m: number;
  len: number;
  data: number[][];
  parity: number[][];
  generator: number[][];
  col: number;
  terms: { row: number; parts: { c: number; d: number; p: number }[]; result: number }[];
  lost: number[];
  decode:
    | { ok: true; chosen: number[]; matrix: number[]; inverse: number[]; missing: number[]; data: number[][]; text: string; matches: boolean }
    | { ok: false; have: number; need: number };
}

const state = { text: 'OTA rocks! v1.1', k: 4, m: 2, lost: new Set<number>([1]), col: 0 };

export function mountProtect(fig: HTMLElement): void {
  const textIn = h('input', { class: 'mono', attrs: { value: state.text, maxlength: 32, 'aria-label': ct('c5.text') } });
  const shardsBox = h('div', { class: 'rs-shards' });
  const verdict = h('p', { class: 'verdict' });
  const matrices = h('div', { class: 'two-col' });
  const column = h('div', { class: 'rs-column' });
  const colCtl = slider(ct('c5.col'), state.col, 0, 7, 1, (v) => ((state.col = v), render()));
  const colInput = colCtl.querySelector('input') as HTMLInputElement;
  const colOut = colCtl.querySelector('output') as HTMLOutputElement;

  const render = () => {
    for (const i of [...state.lost]) if (i >= state.k + state.m) state.lost.delete(i);
    const r = lesson<RsLesson>('rs', { text: state.text || ' ', k: state.k, m: state.m, lost: [...state.lost], col: state.col });
    state.col = r.col;
    const missing = r.decode.ok ? new Set(r.decode.missing) : new Set<number>();
    const chosen = r.decode.ok ? new Set(r.decode.chosen) : new Set<number>();
    const all = [...r.data, ...r.parity];
    shardsBox.replaceChildren(
      ...all.map((bytes, i) => {
        const isParity = i >= r.k;
        const lost = state.lost.has(i);
        const shown = lost && r.decode.ok && !isParity ? r.decode.data[i] : bytes;
        const cls = ['rs-shard', isParity ? 'parity' : 'data', lost ? 'lost' : '', missing.has(i) ? 'rebuilt' : '', chosen.has(i) ? 'used' : '']
          .filter(Boolean)
          .join(' ');
        return h(
          'button',
          {
            class: cls,
            attrs: { type: 'button', 'aria-pressed': lost },
            on: {
              click: () => {
                if (state.lost.has(i)) state.lost.delete(i);
                else state.lost.add(i);
                render();
              },
            },
          },
          h('b', { text: isParity ? `p${i - r.k}` : `d${i}` }),
          h('span', { class: 'mono hexes', text: shown.map(hex2).join(' ') }),
          isParity ? '' : h('span', { class: 'mono glyphs', text: shown.map(glyph).join('') }),
          lost ? h('span', { class: 'tag', text: missing.has(i) ? ct('c5.rebuilt') : ct('c5.lost') }) : '',
        );
      }),
    );
    if (r.decode.ok) {
      verdict.className = 'verdict good';
      verdict.textContent = ct('c5.ok', { used: r.decode.chosen.map((i) => (i < r.k ? `d${i}` : `p${i - r.k}`)).join(', '), text: r.decode.text });
      matrices.replaceChildren(
        h('div', null, h('h4', { text: ct('c5.matrix') }), matrix(r.generator, chosen, state.lost, r.k)),
        h('div', null, h('h4', { text: ct('c5.inverse') }), matrix(chunk(r.decode.inverse, r.k), new Set(), new Set(), r.k)),
      );
    } else {
      verdict.className = 'verdict bad';
      verdict.textContent = ct('c5.fail', { have: r.decode.have, need: r.decode.need });
      matrices.replaceChildren(h('div', null, h('h4', { text: ct('c5.matrix') }), matrix(r.generator, new Set(), state.lost, r.k)));
    }
    // One byte column, worked out.
    colInput.max = String(r.len - 1);
    colInput.value = String(r.col);
    colOut.textContent = String(r.col);
    const t0 = r.terms[0];
    column.replaceChildren(
      ...r.terms.map((t, i) =>
        h('p', {
          class: 'mono eq',
          text: ct('c5.eq', {
            i,
            c: r.col,
            terms: t.parts.map((p) => `${hex2(p.c)}·${hex2(p.d)}`).join(' ⊕ '),
            r: hex2(t.result),
          }),
        }),
      ),
      t0
        ? h(
            'div',
            { class: 'xor-table' },
            h('p', { class: 'muted', text: ct('c5.xor') }),
            ...t0.parts.map((p) => xorRow(`${hex2(p.c)}·${hex2(p.d)} =`, p.p)),
            xorRow(`p0[${r.col}] =`, t0.result, true),
          )
        : '',
    );
  };

  const onText = debounce(() => {
    state.text = textIn.value;
    render();
  }, 150);
  textIn.addEventListener('input', onText);
  fig.append(
    h(
      'div',
      { class: 'controls-row' },
      h('label', { class: 'ctl' }, h('span', { text: ct('c5.text') }), textIn),
      slider(ct('c5.k'), state.k, 2, 6, 1, (v) => ((state.k = v), render())),
      slider(ct('c5.m'), state.m, 1, 4, 1, (v) => ((state.m = v), render())),
    ),
    h('h4', { text: ct('c5.shards') }),
    shardsBox,
    verdict,
    matrices,
    h('h4', { text: ct('c5.column') }),
    colCtl,
    column,
  );
  render();
}

function chunk(flat: number[], k: number): number[][] {
  return Array.from({ length: flat.length / k }, (_, r) => flat.slice(r * k, r * k + k));
}

function matrix(rows: number[][], used: Set<number>, lost: Set<number>, k: number): HTMLElement {
  return h(
    'div',
    { class: 'matrix mono' },
    ...rows.map((row, r) =>
      h(
        'div',
        { class: `mrow${used.has(r) ? ' used' : ''}${lost.has(r) ? ' lost' : ''}${r === k ? ' sep' : ''}` },
        ...row.map((v) => h('span', { text: hex2(v) })),
      ),
    ),
  );
}

function xorRow(label: string, v: number, total = false): HTMLElement {
  return h(
    'div',
    { class: `xor-row${total ? ' total' : ''}` },
    h('span', { class: 'mono', text: label }),
    h('span', { class: 'bitcells' }, ...bits8(v).split('').map((c) => h('span', { class: `bit${c === '1' ? ' one' : ''}`, text: c }))),
  );
}
