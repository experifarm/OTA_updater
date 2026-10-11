// Chapter 1: the pipeline at a glance, with numbers from the real 1 MiB example.

import { presetLog } from '../engine.ts';
import { fmtBytes } from '../i18n.ts';
import { ct } from './content.ts';
import { h } from './ui.ts';

export function mountOverview(fig: HTMLElement): void {
  const s = presetLog('clean', 'real').summary;
  const stage = (ch: number, key: string, num: string) =>
    h(
      'a',
      { class: 'stage-card', attrs: { href: `#learn/${ch}` }, style: `--i:${ch}` },
      h('span', { class: 'stage-num', text: String(ch) }),
      h('b', { text: ct(`ov.${key}`) }),
      h('span', { class: 'stage-d', text: ct(`ov.${key}.d`) }),
      h('span', { class: 'stage-n', text: num }),
    );
  const lane = (label: string, cls: string, ...cards: HTMLElement[]) =>
    h('div', { class: `lane ${cls}` }, h('div', { class: 'lane-label', text: label }), h('div', { class: 'lane-cards' }, ...cards));

  fig.append(
    h(
      'div',
      { class: 'pipeline' },
      lane(
        ct('lane.sender'),
        'sender',
        stage(2, 'diff', ct('ov.n.diff', { size: fmtBytes(s.newSize), records: s.records })),
        stage(3, 'compress', ct('ov.n.compress', { raw: fmtBytes(s.rawPatch), patch: fmtBytes(s.patchSize) })),
        stage(4, 'slice', ct('ov.n.slice', { groups: s.groups, k: s.k, s: s.shardSize })),
        stage(5, 'protect', ct('ov.n.protect', { m: s.m })),
        stage(6, 'sign', ct('ov.n.sign', { size: fmtBytes(s.manifestLen) })),
      ),
      lane(ct('lane.air'), 'air', stage(7, 'broadcast', ct('ov.n.broadcast', { packets: s.packetsPerRound }))),
      lane(
        ct('lane.device'),
        'device',
        stage(8, 'receive', ct('ov.n.receive', { k: s.k, n: s.k + s.m })),
        stage(9, 'install', ct('ov.n.install')),
      ),
    ),
    h(
      'div',
      { class: 'row-actions' },
      h('a', { class: 'btn primary', text: `${ct('nav.next')}`, attrs: { href: '#learn/2' } }),
      h('a', { class: 'btn', text: ct('nav.toSandbox'), attrs: { href: '#sandbox' } }),
    ),
  );
}
