// Chapter 9: verify slot B against the signed hash, then switch — or stay on slot A.

import { lesson } from '../engine.ts';
import { ct } from './content.ts';
import { button, h } from './ui.ts';

interface InstallLesson {
  slotA: string;
  slotB: string;
  expected: string;
  actual: string;
  ok: boolean;
  fwVersion: number;
}

// Corrupt lower-case letters (flipping bit 5 makes the change visible as an upper-case letter).
const state = { corrupt: -1, next: 0 };
let letters: number[] = [];

export function mountInstall(fig: HTMLElement): void {
  if (!letters.length) {
    const clean = new TextEncoder().encode(lesson<InstallLesson>('install', {}).slotB);
    letters = [...clean.keys()].filter((i) => clean[i] >= 97 && clean[i] <= 122).filter((_, k) => k % 23 === 7);
  }
  const slots = h('div', { class: 'slots' });
  const hashes = h('div', { class: 'hash-box' });
  const verdict = h('p', { class: 'verdict' });
  const render = () => {
    const r = lesson<InstallLesson>('install', { corrupt: state.corrupt });
    const slotB = h('pre', { class: 'mono slot-text' });
    if (state.corrupt >= 0) {
      const bytes = new TextEncoder().encode(r.slotB);
      const dec = new TextDecoder();
      slotB.append(
        dec.decode(bytes.subarray(0, state.corrupt)),
        h('span', { class: 'flip', text: dec.decode(bytes.subarray(state.corrupt, state.corrupt + 1)) }),
        dec.decode(bytes.subarray(state.corrupt + 1)),
      );
    } else slotB.textContent = r.slotB;
    slots.replaceChildren(
      h(
        'div',
        { class: `slot${r.ok ? '' : ' active'}` },
        h('h4', null, ct('c9.slotA'), r.ok ? '' : h('span', { class: 'badge', text: ct('c9.active') })),
        h('pre', { class: 'mono slot-text', text: r.slotA }),
      ),
      h('div', { class: `switch ${r.ok ? 'go' : 'stay'}`, text: r.ok ? '⟶' : '✕' }),
      h(
        'div',
        { class: `slot${r.ok ? ' active' : ' bad'}` },
        h('h4', null, ct('c9.slotB'), r.ok ? h('span', { class: 'badge', text: ct('c9.active') }) : ''),
        slotB,
      ),
    );
    const row = (label: string, value: string, against: string) =>
      h(
        'div',
        { class: 'hash-row' },
        h('span', { class: 'hash-in', text: label }),
        h('span', { class: 'mono hash' }, ...value.split('').map((c, i) => h('span', { class: c === against[i] ? '' : 'flip', text: c }))),
      );
    hashes.replaceChildren(row(ct('c9.expected'), r.expected, r.expected), row(ct('c9.actual'), r.actual, r.expected));
    verdict.className = `verdict ${r.ok ? 'good' : 'bad'}`;
    verdict.textContent = ct(r.ok ? 'c9.ok' : 'c9.bad');
  };
  fig.append(
    h(
      'div',
      { class: 'row-actions' },
      button(ct('c9.corrupt'), () => {
        state.corrupt = letters[state.next++ % letters.length];
        render();
      }),
      button(ct('c9.repair'), () => {
        state.corrupt = -1;
        render();
      }),
    ),
    slots,
    hashes,
    verdict,
    h(
      'div',
      { class: 'row-actions' },
      h('a', { class: 'btn', text: ct('c9.tryAttacker'), attrs: { href: '#sandbox/attacker' } }),
      h('a', { class: 'btn', text: ct('c9.tryRefused'), attrs: { href: '#sandbox/refused' } }),
    ),
  );
  render();
}
