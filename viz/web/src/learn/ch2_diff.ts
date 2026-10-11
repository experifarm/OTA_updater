// Chapter 2: the diff — provenance of every byte, the edit script, the patch stream, bits, suffix search.

import { lesson } from '../engine.ts';
import { ct } from './content.ts';
import { bits8, debounce, glyph, h, hex2 } from './ui.ts';

interface Seg {
  kind: 'header' | 'ctrl' | 'diff' | 'extra';
  start: number;
  end: number;
  diffLen?: number;
  extraLen?: number;
  seek?: number;
}

interface DiffLesson {
  old: number[];
  new: number[];
  raw: number[];
  segments: Seg[];
  kind: number[];
  src: number[];
  stats: { records: number; diffBytes: number; extraBytes: number; rawLen: number };
}

interface SuffixLesson {
  suffixes: { pos: number; text: string }[];
  steps: { lo: number; hi: number; mid: number; goRight: boolean }[];
  match: { len: number; pos: number; text: string };
}

const dec = new TextDecoder();
const state = { old: '', new: '', selected: -1, word: 'banana', query: 'anab' };

export function mountDiff(fig: HTMLElement): void {
  if (!state.old) {
    const d = lesson<DiffLesson>('diff');
    state.old = dec.decode(new Uint8Array(d.old));
    state.new = dec.decode(new Uint8Array(d.new));
  }
  const oldArea = h('textarea', { class: 'mono', attrs: { rows: 11, spellcheck: false, 'aria-label': ct('c2.old') } });
  const newArea = h('textarea', { class: 'mono', attrs: { rows: 11, spellcheck: false, 'aria-label': ct('c2.new') } });
  oldArea.value = state.old;
  newArea.value = state.new;
  const colored = h('pre', { class: 'bytes mono' });
  const bitsBox = h('div', { class: 'bits-box' });
  const script = h('ol', { class: 'script' });
  const stream = h('div', { class: 'chips' });
  const streamTitle = h('h4');
  const error = h('p', { class: 'error-inline' });

  let d: DiffLesson | null = null;
  let patchPos: number[] = [];

  const recompute = () => {
    try {
      d = lesson<DiffLesson>('diff', { old: state.old, new: state.new });
      error.textContent = '';
    } catch (e) {
      error.textContent = String(e instanceof Error ? e.message : e);
      return;
    }
    patchPos = mapPatchPositions(d);
    if (state.selected >= d.new.length) state.selected = -1;
    renderColored();
    renderScript();
    renderStream();
    renderBits();
  };

  const renderColored = () => {
    if (!d) return;
    colored.replaceChildren(
      ...d.new.map((b, i) =>
        h('span', {
          class: `b k${d!.kind[i]}${i === state.selected ? ' sel' : ''}`,
          text: b === 10 ? '↵\n' : b >= 32 && b < 127 ? String.fromCharCode(b) : '·',
          attrs: { 'data-i': i },
        }),
      ),
    );
  };
  colored.addEventListener('click', (e) => {
    const t = (e.target as HTMLElement).closest('[data-i]') as HTMLElement | null;
    if (!t) return;
    state.selected = Number(t.dataset.i);
    renderColored();
    renderBits();
    renderStream();
  });

  const renderScript = () => {
    if (!d) return;
    const items: HTMLElement[] = [];
    let oldPos = 0;
    let newPos = 0;
    for (const s of d.segments) {
      if (s.kind !== 'ctrl') continue;
      const dl = s.diffLen ?? 0;
      const el = s.extraLen ?? 0;
      const parts: (HTMLElement | string)[] = [];
      if (dl > 0) {
        let adjusted = 0;
        for (let j = 0; j < dl; j++) if (d.kind[newPos + j] === 1) adjusted++;
        parts.push(
          h('span', { class: 'op copy', text: ct('c2.copy', { n: dl, at: oldPos }) }),
          adjusted ? h('span', { class: 'op adj', text: ct('c2.adjusted', { n: adjusted }) }) : '',
        );
      }
      if (el > 0) {
        const lit = d.new.slice(newPos + dl, newPos + dl + el).map(glyph).join('');
        parts.push(h('span', { class: 'op ins', text: ct('c2.insert', { n: el }) }), h('code', { text: `“${lit}”` }));
      }
      oldPos += dl + (s.seek ?? 0);
      newPos += dl + el;
      if (newPos < d.new.length && s.seek) parts.push(h('span', { class: 'op seek', text: ct('c2.seek', { n: s.seek }) }));
      items.push(h('li', null, ...parts));
    }
    script.replaceChildren(...items);
  };

  const renderStream = () => {
    if (!d) return;
    streamTitle.textContent = ct('c2.stream', { n: d.raw.length });
    const kindAt = new Array<string>(d.raw.length);
    for (const s of d.segments) for (let i = s.start; i < s.end; i++) kindAt[i] = s.kind;
    const hi = state.selected >= 0 ? patchPos[state.selected] : -1;
    stream.replaceChildren(
      ...d.raw.map((b, i) =>
        h('span', {
          class: `chip ${kindAt[i]}${kindAt[i] === 'diff' && b === 0 ? ' zero' : ''}${i === hi ? ' sel' : ''}`,
          text: hex2(b),
          title: `#${i} · ${ct(`c2.seg.${kindAt[i]}`)}`,
        }),
      ),
    );
  };

  const renderBits = () => {
    if (!d || state.selected < 0) {
      bitsBox.replaceChildren(h('p', { class: 'muted', text: ct('c2.bits.pick') }));
      return;
    }
    const i = state.selected;
    const n = d.new[i];
    const where = h('p', { class: 'muted', text: ct('c2.bits.where', { n: patchPos[i] }) });
    if (d.kind[i] === 2) {
      bitsBox.replaceChildren(byteRow(ct('c2.bits.newb'), n, null), h('p', { text: ct('c2.bits.extra') }), where);
      return;
    }
    const o = d.old[d.src[i]];
    const diff = (n - o + 256) & 255;
    const flips = bits8(n ^ o).split('').filter((c) => c === '1').length;
    bitsBox.replaceChildren(
      byteRow(`${ct('c2.bits.oldb')} @${d.src[i]}`, o, null),
      byteRow(ct('c2.bits.newb'), n, n ^ o),
      h('p', { class: 'muted', text: ct('c2.bits.flip', { n: flips }) }),
      byteRow(ct('c2.bits.diff'), diff, null, true),
      where,
    );
  };

  const onEdit = debounce(() => {
    state.old = oldArea.value;
    state.new = newArea.value;
    recompute();
  }, 180);
  oldArea.addEventListener('input', onEdit);
  newArea.addEventListener('input', onEdit);

  fig.append(
    h(
      'div',
      { class: 'two-col' },
      h('label', { class: 'field' }, h('span', { text: ct('c2.old') }), oldArea),
      h('label', { class: 'field' }, h('span', { text: ct('c2.new') }), newArea),
    ),
    error,
    h('h4', { text: ct('c2.colored') }),
    h(
      'p',
      { class: 'legend-inline' },
      h('span', { class: 'b k0', text: ct('c2.same') }),
      h('span', { class: 'b k1', text: ct('c2.chg') }),
      h('span', { class: 'b k2', text: ct('c2.ext') }),
    ),
    h('div', { class: 'two-col wide-left' }, colored, h('div', null, h('h4', { text: ct('c2.bits') }), bitsBox)),
    h('h4', { text: ct('c2.script') }),
    script,
    streamTitle,
    h(
      'p',
      { class: 'legend-inline' },
      ...(['header', 'ctrl', 'diff', 'extra'] as const).map((k) => h('span', { class: `chip ${k}`, text: ct(`c2.seg.${k}`) })),
    ),
    stream,
    suffixPanel(),
  );
  recompute();
}

/** Patch-stream position of every new-image byte (where its diff or literal byte is stored). */
function mapPatchPositions(d: DiffLesson): number[] {
  const pos = new Array<number>(d.new.length).fill(-1);
  let newPos = 0;
  for (const s of d.segments) {
    if (s.kind === 'diff' || s.kind === 'extra') {
      for (let j = 0; j < s.end - s.start; j++) pos[newPos + j] = s.start + j;
      newPos += s.end - s.start;
    }
  }
  return pos;
}

function byteRow(label: string, b: number, xor: number | null, diff = false): HTMLElement {
  const bits = bits8(b);
  const flips = xor === null ? '00000000' : bits8(xor);
  return h(
    'div',
    { class: 'byte-row' },
    h('span', { class: 'byte-label', text: label }),
    h('span', { class: 'byte-glyph mono', text: diff ? '' : glyph(b) }),
    h('span', { class: 'byte-hex mono', text: `0x${hex2(b)}` }),
    h(
      'span',
      { class: 'bitcells' },
      ...bits.split('').map((c, k) =>
        h('span', { class: `bit${c === '1' ? ' one' : ''}${flips[k] === '1' ? ' flip' : ''}${diff && b === 0 ? ' zero' : ''}`, text: c }),
      ),
    ),
  );
}

function suffixPanel(): HTMLElement {
  const wordIn = h('input', { class: 'mono', attrs: { value: state.word, maxlength: 40, 'aria-label': ct('c2.sa.word') } });
  const queryIn = h('input', { class: 'mono', attrs: { value: state.query, maxlength: 40, 'aria-label': ct('c2.sa.query') } });
  const table = h('div', { class: 'sa-table' });
  const steps = h('ol', { class: 'sa-steps' });
  const result = h('p', { class: 'sa-result' });
  const render = () => {
    let s: SuffixLesson;
    try {
      s = lesson<SuffixLesson>('suffix', { word: state.word, query: state.query });
    } catch (e) {
      result.textContent = String(e instanceof Error ? e.message : e);
      return;
    }
    const last = s.steps[s.steps.length - 1];
    const finals = last ? [last.goRight ? last.mid : last.lo, last.goRight ? last.hi : last.mid] : [0, s.suffixes.length - 1];
    table.replaceChildren(
      ...s.suffixes.map((x, i) => {
        const probed = s.steps.findIndex((st) => st.mid === i);
        const isMatch = x.pos === s.match.pos && s.match.len > 0 && finals.includes(i);
        return h(
          'div',
          { class: `sa-row${finals.includes(i) ? ' final' : ''}${isMatch ? ' match' : ''}` },
          h('span', { class: 'sa-i mono', text: `#${i}` }),
          h('span', { class: 'sa-pos mono', text: `@${x.pos}` }),
          h('span', { class: 'sa-text mono', text: x.text === '' ? '∅' : x.text }),
          probed >= 0 ? h('span', { class: 'sa-probe', text: `${probed + 1}` }) : '',
        );
      }),
    );
    steps.replaceChildren(
      ...s.steps.map((st, i) =>
        h('li', {
          text: ct('c2.sa.step', {
            i: i + 1,
            lo: st.lo,
            hi: st.hi,
            mid: st.mid,
            dir: ct(st.goRight ? 'c2.sa.right' : 'c2.sa.left'),
          }),
        }),
      ),
    );
    result.textContent = ct('c2.sa.result', { text: s.match.text, len: s.match.len, pos: s.match.pos });
  };
  const onInput = debounce(() => {
    state.word = wordIn.value;
    state.query = queryIn.value;
    render();
  }, 120);
  wordIn.addEventListener('input', onInput);
  queryIn.addEventListener('input', onInput);
  render();
  return h(
    'section',
    { class: 'sub' },
    h('h4', { text: ct('c2.sa') }),
    h('p', { class: 'muted', text: ct('c2.sa.explain') }),
    h(
      'div',
      { class: 'inline-fields' },
      h('label', null, h('span', { text: ct('c2.sa.word') }), wordIn),
      h('label', null, h('span', { text: ct('c2.sa.query') }), queryIn),
    ),
    h('div', { class: 'two-col' }, table, h('div', null, steps, result)),
  );
}
