// Chapter 6: fingerprints (SHA-256 avalanche) and the Ed25519-sealed manifest under tampering.

import { lesson } from '../engine.ts';
import { ct } from './content.ts';
import { button, debounce, h, slider } from './ui.ts';

interface HashLesson {
  text: string;
  flipped: string;
  h1: string;
  h2: string;
  differingBits: number;
}

interface SignLesson {
  action: string;
  fields: { name: string; value: string; changed: boolean }[];
  signature: string;
  sigChanged: boolean;
  publicKey: string;
  verified: boolean;
  error: string | null;
}

const ACTIONS = ['none', 'fw', 'hash', 'key', 'resign'] as const;
const state = { text: 'firmware v1.1.0', bit: 3, action: 'none' as (typeof ACTIONS)[number] };

export function mountSign(fig: HTMLElement): void {
  // --- avalanche
  const textIn = h('input', { class: 'mono', attrs: { value: state.text, maxlength: 64, 'aria-label': ct('c6.hash') } });
  const hashBox = h('div', { class: 'hash-box' });
  const renderHash = () => {
    const r = lesson<HashLesson>('hash', { text: state.text, bit: state.bit });
    const diff = (a: string, b: string) =>
      a.split('').map((c, i) => h('span', { class: c === b[i] ? '' : 'flip', text: c }));
    hashBox.replaceChildren(
      h('div', { class: 'hash-row' }, h('span', { class: 'mono hash-in', text: r.text }), h('span', { class: 'mono hash' }, ...diff(r.h1, r.h1))),
      h(
        'div',
        { class: 'hash-row' },
        h('span', { class: 'mono hash-in', text: r.flipped }),
        h('span', { class: 'mono hash' }, ...diff(r.h2, r.h1)),
      ),
      h('p', { class: 'big-stat', text: ct('c6.differ', { n: r.differingBits }) }),
    );
  };
  const bitCtl = slider(
    ct('c6.flip'),
    state.bit,
    -1,
    Math.max(7, state.text.length * 8 - 1),
    1,
    (v) => ((state.bit = v), renderHash()),
    (v) => (v < 0 ? ct('c6.noflip') : `#${v}`),
  );
  const bitInput = bitCtl.querySelector('input') as HTMLInputElement;
  textIn.addEventListener(
    'input',
    debounce(() => {
      state.text = textIn.value;
      bitInput.max = String(Math.max(0, state.text.length * 8 - 1));
      renderHash();
    }, 120),
  );

  // --- manifest
  const actions = h('div', { class: 'actions' });
  const card = h('div', { class: 'manifest-card' });
  const renderSign = () => {
    const r = lesson<SignLesson>('sign', { action: state.action });
    actions.replaceChildren(
      ...ACTIONS.map((a) => {
        const b = button(ct(`c6.a.${a}`), () => ((state.action = a), renderSign()));
        b.setAttribute('aria-pressed', String(a === state.action));
        return b;
      }),
    );
    card.className = `manifest-card ${r.verified ? 'valid' : 'invalid'}`;
    card.replaceChildren(
      h(
        'dl',
        null,
        ...r.fields.flatMap((f) => [
          h('dt', { text: ct(`f.${f.name}`) }),
          h('dd', { class: `mono${f.changed ? ' changed' : ''}`, text: f.value, title: f.value }),
        ]),
        h('dt', { text: ct('c6.sig') }),
        h('dd', { class: `mono sig${r.sigChanged ? ' changed' : ''}`, text: r.signature, title: r.signature }),
        h('dt', { text: ct('c6.key') }),
        h('dd', { class: 'mono', text: r.publicKey, title: r.publicKey }),
      ),
      h(
        'p',
        { class: `verdict ${r.verified ? 'good' : 'bad'}` },
        h('span', { class: 'seal', text: r.verified ? '✓' : '✗' }),
        ct(r.verified ? 'c6.valid' : 'c6.invalid'),
        r.error ? h('span', { class: 'muted', text: ` (${r.error})` }) : '',
      ),
    );
  };

  fig.append(
    h('h4', { text: ct('c6.hash') }),
    h('div', { class: 'controls-row' }, h('label', { class: 'ctl' }, h('span', { text: ct('c5.text') }), textIn), bitCtl),
    hashBox,
    h('h4', { text: ct('c6.manifest') }),
    actions,
    card,
  );
  renderHash();
  renderSign();
}
