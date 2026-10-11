// Chapter shell: navigation pills, story / try-it / figure / under-the-hood, previous / next.

import { ct } from './content.ts';
import { h } from './ui.ts';
import { mountOverview } from './ch1_overview.ts';
import { mountDiff } from './ch2_diff.ts';
import { mountCompress } from './ch3_compress.ts';
import { mountSlice } from './ch4_slice.ts';
import { mountProtect } from './ch5_protect.ts';
import { mountSign } from './ch6_sign.ts';
import { mountBroadcast } from './ch7_broadcast.ts';
import { mountReceive } from './ch8_receive.ts';
import { mountInstall } from './ch9_install.ts';

/** A mounted figure may return a cleanup function (timers etc.). */
export type Mount = (fig: HTMLElement) => void | (() => void);

interface Chapter {
  hood: number;
  files: string[];
  mount: Mount;
}

export const CHAPTERS: Chapter[] = [
  { hood: 3, files: ['src/', 'viz/wasm/'], mount: mountOverview },
  { hood: 4, files: ['src/suffix.rs', 'src/diff.rs', 'src/patch.rs'], mount: mountDiff },
  { hood: 3, files: ['src/diff.rs', 'src/patch.rs'], mount: mountCompress },
  { hood: 3, files: ['src/slicer.rs', 'src/wire.rs'], mount: mountSlice },
  { hood: 3, files: ['src/gf256.rs', 'src/rs.rs'], mount: mountProtect },
  { hood: 3, files: ['src/manifest.rs'], mount: mountSign },
  { hood: 3, files: ['src/sender.rs', 'src/net.rs'], mount: mountBroadcast },
  { hood: 3, files: ['src/wire.rs', 'src/receiver/core.rs'], mount: mountReceive },
  { hood: 2, files: ['src/receiver/mod.rs'], mount: mountInstall },
];

let cleanup: (() => void) | undefined;

/** Render chapter `n` (1-based) into `root`. */
export function renderChapter(root: HTMLElement, n: number): void {
  cleanup?.();
  cleanup = undefined;
  const idx = Math.min(Math.max(1, n), CHAPTERS.length) - 1;
  const ch = CHAPTERS[idx];
  const id = idx + 1;

  const pills = h(
    'nav',
    { class: 'chapters', attrs: { 'aria-label': ct('nav.learn') } },
    ...CHAPTERS.map((_, i) =>
      h(
        'a',
        {
          class: i === idx ? 'pill current' : 'pill',
          attrs: { href: `#learn/${i + 1}`, 'aria-current': i === idx ? 'page' : false },
        },
        h('b', { text: String(i + 1) }),
        ' ',
        ct(`c${i + 1}.title`).split(/[:：]/)[0],
      ),
    ),
  );

  const fig = h('div', { class: 'figure' });
  const hood = h(
    'details',
    { class: 'hood' },
    h('summary', { text: ct('hood') }),
    h('ul', null, ...Array.from({ length: ch.hood }, (_, i) => h('li', { text: ct(`c${id}.hood${i + 1}`) }))),
    h('p', { class: 'files' }, `${ct('files')}: `, ...ch.files.flatMap((f, i) => [i ? ', ' : '', h('code', { text: f })])),
  );
  const prev = id > 1 ? h('a', { class: 'btn', text: ct('nav.prev'), attrs: { href: `#learn/${id - 1}` } }) : h('span');
  const next =
    id < CHAPTERS.length
      ? h('a', { class: 'btn primary', text: ct('nav.next'), attrs: { href: `#learn/${id + 1}` } })
      : h('a', { class: 'btn primary', text: ct('nav.toSandbox'), attrs: { href: '#sandbox' } });

  const article = h(
    'article',
    { class: 'chapter' },
    h('header', null, h('span', { class: 'num', text: String(id) }), h('h2', { text: ct(`c${id}.title`) })),
    h('div', { class: 'story' }, h('h3', { text: ct('story') }), h('p', { class: 'lead', text: ct(`c${id}.lead`) })),
    h('p', { class: 'hint' }, h('b', { text: `${ct('try')}: ` }), ct(`c${id}.try`)),
    fig,
    h('p', { class: 'engine-note', text: ct('realEngine') }),
    hood,
    h('footer', null, prev, next),
  );
  root.replaceChildren(pills, article);
  try {
    cleanup = ch.mount(fig) ?? undefined;
  } catch (e) {
    fig.append(h('p', { class: 'error-inline', text: String(e instanceof Error ? e.message : e) }));
  }
}

/** Stop timers of the current chapter (when leaving the learn view). */
export function leaveChapter(): void {
  cleanup?.();
  cleanup = undefined;
}
