// Turns the event log into narrated moments (also used as timeline markers).

import { deviceName, fmtBytes, reasonText, t } from './i18n.ts';
import type { ScenarioLog } from './types.ts';

export type Tone = 'info' | 'good' | 'warn' | 'bad';

export interface Moment {
  t: number;
  tone: Tone;
  rx: number | null;
  /** Localized text, rebuilt when the language changes. */
  text: () => string;
}

const MAX_REBUILT_PER_RX = 3;

export function buildMoments(log: ScenarioLog): Moment[] {
  const s = log.summary;
  const out: Moment[] = [];
  const ratio = `${((100 * s.patchSize) / Math.max(1, s.newSize)).toFixed(1)}%`;
  out.push({
    t: 0,
    tone: 'info',
    rx: null,
    text: () =>
      t(s.patchSize >= s.newSize ? 'n.introTiny' : 'n.intro', {
        records: s.records,
        extra: s.extraBytes,
        newSize: fmtBytes(s.newSize),
        patch: fmtBytes(s.patchSize),
        ratio,
      }),
  });
  out.push({ t: 0.1, tone: 'info', rx: null, text: () => t('n.slice', { groups: s.groups, k: s.k, m: s.m, n: s.k + s.m }) });
  out.push({ t: 0.2, tone: 'info', rx: null, text: () => t('n.sign') });
  log.rounds.forEach((rt, i) => {
    out.push({
      t: rt + 0.3,
      tone: 'info',
      rx: null,
      text: () => (i === 0 ? t('n.round1', { packets: s.packetsPerRound }) : t('n.roundN', { r: i + 1 })),
    });
  });
  const firstAttack = log.packets.find((p) => p.src === 1);
  if (firstAttack) out.push({ t: firstAttack.t + 0.4, tone: 'bad', rx: null, text: () => t('n.attacker') });

  log.receivers.forEach((rx, r) => {
    const nm = () => deviceName(rx.spec.name);
    if (rx.spec.joinMs > 0) out.push({ t: rx.spec.joinMs, tone: 'warn', rx: r, text: () => t('n.late', { name: nm() }) });
    const firstLost = rx.deliveries.find((d) => d.f === 'lost');
    if (firstLost) out.push({ t: firstLost.t, tone: 'warn', rx: r, text: () => t('n.lost', { name: nm() }) });
    let rebuilt = 0;
    let crc = false;
    let badSig = false;
    let streaming = false;
    for (const e of rx.events) {
      switch (e.type) {
        case 'manifest_ok':
          out.push({ t: e.t, tone: 'good', rx: r, text: () => t('n.manifestOk', { name: nm() }) });
          break;
        case 'manifest_bad_sig':
          if (!badSig) out.push({ t: e.t, tone: 'bad', rx: r, text: () => t('n.badSig', { name: nm() }) });
          badSig = true;
          break;
        case 'crc_bad':
          if (!crc) out.push({ t: e.t, tone: 'warn', rx: r, text: () => t('n.crc', { name: nm() }) });
          crc = true;
          break;
        case 'decoded':
          if (!e.ok) {
            const g = e.g;
            out.push({ t: e.t, tone: 'bad', rx: r, text: () => t('n.hashBad', { name: nm(), g }) });
          } else if (e.missing.length > 0 && rebuilt < MAX_REBUILT_PER_RX) {
            rebuilt++;
            const g = e.g;
            const n = e.missing.length;
            out.push({ t: e.t, tone: 'good', rx: r, text: () => t('n.rebuilt', { name: nm(), g, n }) });
          }
          break;
        case 'write':
          if (!streaming) out.push({ t: e.t, tone: 'info', rx: r, text: () => t('n.stream', { name: nm() }) });
          streaming = true;
          break;
        case 'installed':
          out.push({ t: e.t, tone: 'good', rx: r, text: () => t('n.installed', { name: nm() }) });
          break;
        case 'rejected': {
          const reason = e.reason;
          out.push({ t: e.t, tone: 'bad', rx: r, text: () => t('n.rejected', { name: nm(), reason: reasonText(reason) }) });
          break;
        }
        case 'failed': {
          const reason = e.reason;
          out.push({ t: e.t, tone: 'bad', rx: r, text: () => t('n.failed', { name: nm(), reason: reasonText(reason) }) });
          break;
        }
        default:
          break;
      }
    }
  });
  out.sort((a, b) => a.t - b.t);
  return out;
}
