// Canvas renderer: a pure function of (log, receiver states at T, T).

import { deviceName, fmtBytes, reasonText, t } from './i18n.ts';
import { CELL_BUFFERED, CELL_EMPTY, CELL_PAD, CELL_REBUILT, CELL_STORED, RxState, lastPacketAt } from './model.ts';
import type { Delivery, PacketRec, ScenarioLog } from './types.ts';

export const W = 1280;
export const H = 760;

export const C = {
  bg: '#0b1020',
  panel: '#121a2f',
  cell: '#18213a',
  grid: '#26324f',
  text: '#e6ecff',
  dim: '#8a97b8',
  old: '#4c8dff',
  neu: '#a970ff',
  extra: '#ff9f43',
  good: '#3ddc97',
  bad: '#ff5d6c',
  warn: '#ffc857',
  gold: '#f5c542',
  attacker: '#c0392b',
};

const FONT = 'system-ui, -apple-system, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif';

export function groupHue(g: number): number {
  return (g * 137.508) % 360; // golden angle: neighbouring groups get distinct hues
}

export function groupColor(g: number, alpha = 1, light = 62): string {
  return `hsla(${groupHue(g).toFixed(1)}, 72%, ${light}%, ${alpha})`;
}

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Pt {
  x: number;
  y: number;
}

const STRIP = { x0: 28, x1: 640 };
const TILES = { x0: 690, x1: 1252, y0: 50, y1: 172 };
const SENDER: Pt = { x: 74, y: 440 };
const ATTACKER: Pt = { x: 74, y: 690 };
const CARDS = { x: 820, w: 436, top: 214, bottom: H - 14, gap: 10 };

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, r);
}

function bezier(p0: Pt, p1: Pt, p2: Pt, u: number): Pt {
  const a = (1 - u) * (1 - u);
  const b = 2 * (1 - u) * u;
  const c = u * u;
  return { x: a * p0.x + b * p1.x + c * p2.x, y: a * p0.y + b * p1.y + c * p2.y };
}

function ease(u: number): number {
  return u < 0.5 ? 2 * u * u : 1 - (-2 * u + 2) ** 2 / 2;
}

export class Renderer {
  private readonly ctx: CanvasRenderingContext2D;
  private scale = 1;
  private offX = 0;
  private offY = 0;
  reduceMotion = false;

  constructor(private readonly canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('Canvas 2D is not available');
    this.ctx = ctx;
    this.reduceMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
  }

  resize(): void {
    const dpr = window.devicePixelRatio || 1;
    const rect = this.canvas.getBoundingClientRect();
    this.canvas.width = Math.max(1, Math.round(rect.width * dpr));
    this.canvas.height = Math.max(1, Math.round(rect.height * dpr));
    this.scale = Math.min(this.canvas.width / W, this.canvas.height / H);
    this.offX = (this.canvas.width - W * this.scale) / 2;
    this.offY = (this.canvas.height - H * this.scale) / 2;
  }

  cardBoxes(count: number): Box[] {
    const h = (CARDS.bottom - CARDS.top - CARDS.gap * (count - 1)) / count;
    return Array.from({ length: count }, (_, i) => ({ x: CARDS.x, y: CARDS.top + i * (h + CARDS.gap), w: CARDS.w, h }));
  }

  draw(log: ScenarioLog, rxs: RxState[], byPacket: Delivery[][][], T: number): void {
    const ctx = this.ctx;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = C.bg;
    ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
    ctx.setTransform(this.scale, 0, 0, this.scale, this.offX, this.offY);
    const cards = this.cardBoxes(rxs.length);
    this.drawSender(log, T);
    this.drawAir(log, cards, byPacket, T);
    rxs.forEach((rx, r) => this.drawCard(log, rx, cards[r], T));
  }

  // ------------------------------------------------------------------ sender band

  private drawSender(log: ScenarioLog, T: number): void {
    const ctx = this.ctx;
    const s = log.summary;
    const w = STRIP.x1 - STRIP.x0;
    const oldX = (off: number) => STRIP.x0 + (off / Math.max(1, s.oldSize)) * w;
    const newX = (off: number) => STRIP.x0 + (off / Math.max(1, s.newSize)) * w;
    const yOld = 46;
    const yNew = 130;
    const sh = 16;

    ctx.font = `600 13px ${FONT}`;
    ctx.fillStyle = C.text;
    ctx.textBaseline = 'alphabetic';
    ctx.fillText(`${t('band.old')} · ${fmtBytes(s.oldSize)}`, STRIP.x0, yOld - 8);
    ctx.fillText(`${t('band.new')} · ${fmtBytes(s.newSize)}`, STRIP.x0, yNew + sh + 18);

    // Strips.
    ctx.fillStyle = 'rgba(76,141,255,0.45)';
    ctx.fillRect(STRIP.x0, yOld, w, sh);
    ctx.fillStyle = 'rgba(169,112,255,0.28)';
    ctx.fillRect(STRIP.x0, yNew, w, sh);

    // Ribbons: which old bytes each new region is built from.
    for (const [newOff, oldOff, diffLen] of log.records) {
      if (diffLen === 0) continue;
      ctx.beginPath();
      ctx.moveTo(oldX(oldOff), yOld + sh);
      ctx.lineTo(oldX(oldOff + diffLen), yOld + sh);
      ctx.lineTo(newX(newOff + diffLen), yNew);
      ctx.lineTo(newX(newOff), yNew);
      ctx.closePath();
      ctx.fillStyle = 'rgba(140,130,255,0.16)';
      ctx.fill();
    }
    // Changed bytes (diff ≠ 0) and brand-new bytes on the new strip.
    const hb = log.heat.buckets;
    for (let b = 0; b < hb; b++) {
      const x = STRIP.x0 + (b / hb) * w;
      const bw = w / hb + 0.5;
      const ch = log.heat.changed[b];
      if (ch > 0) {
        ctx.fillStyle = `rgba(214,190,255,${Math.min(1, 0.25 + Math.sqrt(ch))})`;
        ctx.fillRect(x, yNew, bw, sh);
      }
      const ex = log.heat.extra[b];
      if (ex > 0) {
        ctx.fillStyle = `rgba(255,159,67,${Math.min(1, 0.35 + ex)})`;
        ctx.fillRect(x, yNew + sh * (1 - Math.min(1, ex * 2)), bw, sh * Math.min(1, ex * 2));
      }
    }
    // Mini legend under the strips.
    ctx.font = `12px ${FONT}`;
    const ly = yNew + sh + 38;
    ctx.fillStyle = 'rgba(214,190,255,0.9)';
    ctx.fillRect(STRIP.x0, ly - 9, 14, 10);
    ctx.fillStyle = C.dim;
    ctx.fillText(t('band.diff'), STRIP.x0 + 20, ly);
    const off2 = STRIP.x0 + 30 + ctx.measureText(t('band.diff')).width;
    ctx.fillStyle = C.extra;
    ctx.fillRect(off2, ly - 9, 14, 10);
    ctx.fillStyle = C.dim;
    ctx.fillText(t('band.extra'), off2 + 20, ly);
    const sizes = t('card.sizes', {
      old: fmtBytes(s.oldSize),
      new: fmtBytes(s.newSize),
      raw: fmtBytes(s.rawPatch),
      patch: fmtBytes(s.patchSize),
      z: s.compressed ? ' (zstd)' : '',
    });
    ctx.fillText(sizes, TILES.x0, TILES.y1 + 24);

    this.drawTiles(log, T);
  }

  private drawTiles(log: ScenarioLog, T: number): void {
    const ctx = this.ctx;
    const s = log.summary;
    const G = s.groups;
    const n = s.k + s.m;
    const colW = Math.min(28, (TILES.x1 - TILES.x0) / G);
    const tileW = Math.max(2, colW - (colW > 8 ? 3 : 1));
    const rowH = Math.min(16, (TILES.y1 - TILES.y0) / n);
    const tileH = Math.max(2, rowH - 2);
    const x0 = TILES.x0;

    ctx.font = `600 13px ${FONT}`;
    ctx.fillStyle = C.text;
    ctx.fillText(`${t('band.patch')} · ${t('band.groups', { groups: G, k: s.k, m: s.m })}`, x0, TILES.y0 - 12);

    // Which tiles were just sent?
    const lastP = lastPacketAt(log, T);
    const flash = new Map<number, number>();
    let round = 0;
    for (let p = lastP; p >= 0 && T - log.packets[p].t < 500; p--) {
      const pk = log.packets[p];
      if (pk.kind === 1) flash.set(pk.g * n + pk.i, Math.max(flash.get(pk.g * n + pk.i) ?? 0, 1 - (T - pk.t) / 500));
    }
    if (lastP >= 0) round = log.packets[lastP].r + 1;

    for (let g = 0; g < G; g++) {
      const real = log.groups[g].real;
      for (let i = 0; i < n; i++) {
        const x = x0 + g * colW;
        const y = TILES.y0 + i * rowH + (i >= s.k ? 4 : 0);
        if (i < s.k && i >= real) {
          ctx.setLineDash([2, 2]);
          ctx.strokeStyle = C.grid;
          ctx.strokeRect(x + 0.5, y + 0.5, tileW - 1, tileH - 1);
          ctx.setLineDash([]);
          continue;
        }
        ctx.fillStyle = groupColor(g, i < s.k ? 0.85 : 0.55, i < s.k ? 62 : 48);
        ctx.fillRect(x, y, tileW, tileH);
        if (i >= s.k && tileW > 5) this.stripes(x, y, tileW, tileH);
        const f = flash.get(g * n + i);
        if (f) {
          ctx.fillStyle = `rgba(255,255,255,${0.8 * f})`;
          ctx.fillRect(x - 1, y - 1, tileW + 2, tileH + 2);
        }
      }
    }
    if (round > 0) {
      ctx.font = `600 12px ${FONT}`;
      ctx.fillStyle = C.warn;
      ctx.textAlign = 'right';
      ctx.fillText(t('band.round', { r: round }), TILES.x1, TILES.y0 - 12);
      ctx.textAlign = 'left';
    }
  }

  private stripes(x: number, y: number, w: number, h: number): void {
    const ctx = this.ctx;
    ctx.save();
    ctx.beginPath();
    ctx.rect(x, y, w, h);
    ctx.clip();
    ctx.strokeStyle = 'rgba(11,16,32,0.55)';
    ctx.lineWidth = 1.5;
    for (let d = -h; d < w; d += 5) {
      ctx.beginPath();
      ctx.moveTo(x + d, y + h);
      ctx.lineTo(x + d + h, y);
      ctx.stroke();
    }
    ctx.restore();
  }

  // ------------------------------------------------------------------ air

  private inbox(card: Box): Pt {
    return { x: card.x - 8, y: card.y + card.h / 2 };
  }

  private drawAir(log: ScenarioLog, cards: Box[], byPacket: Delivery[][][], T: number): void {
    const s = log.summary;
    const lastP = lastPacketAt(log, T);
    const hasAttacker = log.packets.some((p) => p.src === 1);

    // Antenna with pulses.
    const since = lastP >= 0 ? T - log.packets[lastP].t : 1e9;
    this.antenna(SENDER, C.old, t('band.sender'), since);
    const ctx = this.ctx;
    ctx.font = `12px ${FONT}`;
    ctx.fillStyle = 'rgba(138,151,184,0.75)';
    ctx.textAlign = 'center';
    ctx.fillText(t('air.label'), (SENDER.x + CARDS.x) / 2, SENDER.y + 120);
    ctx.textAlign = 'left';
    if (hasAttacker) {
      let lastA = -1;
      for (let p = lastP; p >= 0; p--)
        if (log.packets[p].src === 1) {
          lastA = p;
          break;
        }
      this.antenna(ATTACKER, C.attacker, t('band.attacker'), lastA >= 0 ? T - log.packets[lastA].t : 1e9);
    }

    // Packets in flight.
    const window = s.latencyMs + 10 * s.intervalMs + 700;
    const first = lastPacketAt(log, T - window) + 1;
    for (let p = first; p <= lastP; p++) {
      const pk = log.packets[p];
      const src = pk.src === 1 ? ATTACKER : SENDER;
      cards.forEach((card, r) => {
        const ds = byPacket[r][p];
        ds.forEach((d, j) => this.flight(log, pk, d, j, src, this.inbox(card), r, T));
      });
    }
  }

  private antenna(at: Pt, color: string, label: string, since: number): void {
    const ctx = this.ctx;
    ctx.strokeStyle = color;
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(at.x, at.y + 36);
    ctx.lineTo(at.x, at.y - 10);
    ctx.moveTo(at.x - 16, at.y + 36);
    ctx.lineTo(at.x, at.y + 4);
    ctx.lineTo(at.x + 16, at.y + 36);
    ctx.stroke();
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(at.x, at.y - 14, 6, 0, Math.PI * 2);
    ctx.fill();
    if (!this.reduceMotion && since < 900) {
      for (let k = 0; k < 3; k++) {
        const u = (since / 900 + k / 3) % 1;
        ctx.strokeStyle = color.startsWith('#') ? `${color}${Math.round((1 - u) * 200).toString(16).padStart(2, '0')}` : color;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.arc(at.x, at.y - 14, 10 + u * 34, -Math.PI / 3, Math.PI / 3);
        ctx.stroke();
      }
    }
    ctx.lineWidth = 1;
    ctx.font = `600 13px ${FONT}`;
    ctx.fillStyle = C.text;
    ctx.textAlign = 'center';
    ctx.fillText(label, at.x, at.y + 56);
    ctx.textAlign = 'left';
  }

  private flight(
    log: ScenarioLog,
    pk: PacketRec,
    d: Delivery,
    copy: number,
    src: Pt,
    dst: Pt,
    r: number,
    T: number,
  ): void {
    const start = pk.t;
    const end = d.t;
    if (T < start) return;
    const ctrl = { x: (src.x + dst.x) / 2, y: Math.min(src.y, dst.y) - 70 - r * 12 };
    const dy = copy * 7;
    if (d.f === 'lost') {
      if (T <= end) {
        const u = 0.55 * ease(Math.min(1, (T - start) / Math.max(1, end - start)));
        const p = bezier(src, ctrl, dst, u);
        this.packet(log, pk, p.x, p.y + dy, 1, 'normal');
      } else if (T - end < 520) {
        const f = (T - end) / 520;
        const p = bezier(src, ctrl, dst, 0.55);
        this.packet(log, pk, p.x, p.y + f * 70, 1 - f, 'lost');
      }
      return;
    }
    if (T <= end) {
      const u = ease(Math.min(1, (T - start) / Math.max(1, end - start)));
      const p = bezier(src, ctrl, dst, u);
      const jitter = d.f === 'corrupt' && !this.reduceMotion ? Math.sin(T / 23 + pk.t) * 2 : 0;
      this.packet(log, pk, p.x + jitter, p.y + dy, d.f === 'off' ? 0.25 : 1, d.f === 'corrupt' ? 'corrupt' : d.f === 'late' ? 'late' : 'normal');
    } else if (d.f === 'corrupt' && T - end < 900) {
      const f = (T - end) / 900;
      const ctx = this.ctx;
      ctx.font = `700 12px ${FONT}`;
      ctx.fillStyle = `rgba(255,93,108,${1 - f})`;
      ctx.textAlign = 'right';
      ctx.fillText('CRC ✗', dst.x - 6, dst.y - 8 - f * 14 + dy);
      ctx.textAlign = 'left';
    }
  }

  private packet(log: ScenarioLog, pk: PacketRec, x: number, y: number, alpha: number, style: 'normal' | 'lost' | 'corrupt' | 'late'): void {
    const ctx = this.ctx;
    const k = log.summary.k;
    ctx.globalAlpha = Math.max(0, Math.min(1, alpha));
    if (pk.kind === 0 || pk.kind === 2) {
      // Manifest fragment: an envelope with a seal.
      ctx.fillStyle = pk.kind === 0 ? C.gold : C.attacker;
      roundRect(ctx, x - 8, y - 6, 16, 12, 2);
      ctx.fill();
      ctx.strokeStyle = 'rgba(11,16,32,0.7)';
      ctx.beginPath();
      ctx.moveTo(x - 8, y - 6);
      ctx.lineTo(x, y + 1);
      ctx.lineTo(x + 8, y - 6);
      ctx.stroke();
    } else if (pk.kind === 3) {
      ctx.fillStyle = C.attacker;
      roundRect(ctx, x - 6, y - 6, 12, 12, 2);
      ctx.fill();
      ctx.fillStyle = '#fff';
      ctx.font = `700 10px ${FONT}`;
      ctx.textAlign = 'center';
      ctx.fillText('!', x, y + 4);
      ctx.textAlign = 'left';
    } else {
      ctx.fillStyle = groupColor(pk.g, 1, pk.i < k ? 62 : 48);
      roundRect(ctx, x - 6, y - 6, 12, 12, 2);
      ctx.fill();
      if (pk.i >= k) this.stripes(x - 6, y - 6, 12, 12);
    }
    if (style === 'corrupt') {
      ctx.strokeStyle = C.bad;
      ctx.lineWidth = 2;
      roundRect(ctx, x - 8, y - 8, 16, 16, 3);
      ctx.stroke();
      ctx.lineWidth = 1;
    } else if (style === 'late') {
      ctx.strokeStyle = C.warn;
      ctx.setLineDash([2, 2]);
      roundRect(ctx, x - 8, y - 8, 16, 16, 3);
      ctx.stroke();
      ctx.setLineDash([]);
    } else if (style === 'lost') {
      ctx.strokeStyle = C.bad;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(x - 6, y - 6);
      ctx.lineTo(x + 6, y + 6);
      ctx.moveTo(x + 6, y - 6);
      ctx.lineTo(x - 6, y + 6);
      ctx.stroke();
      ctx.lineWidth = 1;
    }
    ctx.globalAlpha = 1;
  }

  // ------------------------------------------------------------------ receiver cards

  private drawCard(log: ScenarioLog, rx: RxState, box: Box, T: number): void {
    const ctx = this.ctx;
    const spec = rx.rx.spec;
    const done = rx.status === 'installed';
    const bad = rx.status === 'rejected' || rx.status === 'failed';
    roundRect(ctx, box.x, box.y, box.w, box.h, 10);
    ctx.fillStyle = C.panel;
    ctx.fill();
    ctx.strokeStyle = done ? C.good : bad ? C.bad : C.grid;
    ctx.lineWidth = done || bad ? 2 : 1;
    ctx.stroke();
    ctx.lineWidth = 1;

    // Header: name, status, channel weather.
    ctx.font = `700 14px ${FONT}`;
    ctx.fillStyle = rx.status === 'asleep' ? C.dim : C.text;
    const shown = deviceName(spec.name);
    ctx.fillText(shown, box.x + 12, box.y + 20);
    const nameW = ctx.measureText(shown).width;
    ctx.font = `12px ${FONT}`;
    let status: string;
    switch (rx.status) {
      case 'asleep':
        status = t('rx.asleep');
        break;
      case 'listening':
        status = t('rx.listening');
        break;
      case 'receiving':
        status = t('rx.receiving', { done: rx.completed, total: rx.G });
        break;
      case 'installed':
        status = t('rx.installed', { fw: log.manifest.fwVersion });
        break;
      default:
        status = `${t(rx.status === 'rejected' ? 'rx.rejected' : 'rx.failed')}: ${reasonText(rx.reason)}`;
    }
    ctx.fillStyle = done ? C.good : bad ? C.bad : C.dim;
    const weather = this.weather(spec);
    ctx.textAlign = 'right';
    ctx.fillStyle = C.dim;
    ctx.fillText(weather, box.x + box.w - 12, box.y + 20);
    const weatherW = ctx.measureText(weather).width;
    ctx.textAlign = 'left';
    ctx.fillStyle = done ? C.good : bad ? C.bad : C.dim;
    this.fitText(status, box.x + 20 + nameW, box.y + 20, box.w - 44 - nameW - weatherW);

    // Manifest seal.
    const seal = { x: box.x + 24, y: box.y + 32 + (box.h - 32 - 26) / 2 };
    this.seal(rx, seal, T);

    // Shard grid: one column per group, one row per shard (data above the line, parity below).
    const gx = box.x + 46;
    const gw = box.w - 58;
    const top = box.y + 30;
    const gh = box.h - 30 - 30;
    const colW = gw / rx.G;
    const rowH = Math.min(14, (gh - 3) / rx.n);
    const gridH = rowH * rx.n + 3;
    const gy = top + (gh - gridH) / 2;
    const pad = colW > 6 ? 1.5 : 0.5;
    for (let g = 0; g < rx.G; g++) {
      const x = gx + g * colW;
      const doneG = rx.groupDone[g] === 1;
      for (let i = 0; i < rx.n; i++) {
        const idx = g * rx.n + i;
        const y = gy + i * rowH + (i >= rx.k ? 3 : 0);
        const cw = colW - 2 * pad;
        const chh = rowH - 2 * pad;
        const v = rx.cells[idx];
        const age = T - rx.cellT[idx];
        switch (v) {
          case CELL_EMPTY:
            ctx.fillStyle = C.cell;
            ctx.fillRect(x + pad, y + pad, cw, chh);
            break;
          case CELL_BUFFERED:
            ctx.fillStyle = groupColor(g, 0.3);
            ctx.fillRect(x + pad, y + pad, cw, chh);
            break;
          case CELL_STORED:
          case CELL_REBUILT:
            ctx.fillStyle = groupColor(g, doneG ? 0.95 : 0.8, i < rx.k ? 60 : 46);
            ctx.fillRect(x + pad, y + pad, cw, chh);
            if (i >= rx.k && cw > 5) this.stripes(x + pad, y + pad, cw, chh);
            if (v === CELL_REBUILT) {
              ctx.strokeStyle = age < 900 ? `rgba(255,255,255,${1 - age / 1800})` : 'rgba(255,255,255,0.75)';
              ctx.lineWidth = age < 900 ? 2 : 1;
              ctx.strokeRect(x + pad + 0.5, y + pad + 0.5, cw - 1, chh - 1);
              ctx.lineWidth = 1;
            }
            break;
          case CELL_PAD:
            ctx.strokeStyle = C.grid;
            ctx.setLineDash([2, 2]);
            ctx.strokeRect(x + pad + 0.5, y + pad + 0.5, cw - 1, chh - 1);
            ctx.setLineDash([]);
            break;
          default:
            break;
        }
        if (age >= 0 && age < 260 && v !== CELL_PAD && v !== CELL_EMPTY) {
          ctx.fillStyle = `rgba(255,255,255,${0.7 * (1 - age / 260)})`;
          ctx.fillRect(x + pad, y + pad, cw, chh);
        }
      }
      // Group outcome.
      const failAge = T - rx.groupFailT[g];
      if (failAge >= 0 && failAge < 1400) {
        ctx.fillStyle = `rgba(255,93,108,${0.55 * (1 - failAge / 1400)})`;
        ctx.fillRect(x, gy, colW, gridH);
      }
      if (doneG) {
        const a = T - rx.groupDoneT[g];
        ctx.strokeStyle = a < 600 ? `rgba(61,220,151,${1 - a / 1200})` : 'rgba(61,220,151,0.55)';
        ctx.lineWidth = a < 600 ? 2 : 1;
        ctx.strokeRect(x + 0.5, gy - 1.5, colW - 1, gridH + 2);
        ctx.lineWidth = 1;
      }
    }
    // Data/parity separator.
    ctx.strokeStyle = 'rgba(138,151,184,0.35)';
    ctx.beginPath();
    ctx.moveTo(gx, gy + rx.k * rowH + 1.5);
    ctx.lineTo(gx + gw, gy + rx.k * rowH + 1.5);
    ctx.stroke();

    this.writeStrip(log, rx, { x: gx, y: box.y + box.h - 22, w: gw, h: 10 }, T);

    // Inbox marker.
    const ib = this.inbox(box);
    ctx.fillStyle = rx.status === 'asleep' ? C.grid : C.dim;
    ctx.beginPath();
    ctx.arc(ib.x + 4, ib.y, 3, 0, Math.PI * 2);
    ctx.fill();
  }

  private seal(rx: RxState, at: Pt, T: number): void {
    const ctx = this.ctx;
    const badAge = T - rx.badSigT;
    ctx.lineWidth = 2;
    if (rx.manifest === 'ok') {
      const a = T - rx.manifestT;
      const grow = a < 400 ? 1 + 0.4 * (1 - a / 400) : 1;
      ctx.fillStyle = C.gold;
      ctx.beginPath();
      ctx.arc(at.x, at.y, 11 * grow, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = C.bg;
      ctx.beginPath();
      ctx.moveTo(at.x - 5, at.y);
      ctx.lineTo(at.x - 1, at.y + 4);
      ctx.lineTo(at.x + 6, at.y - 4);
      ctx.stroke();
    } else {
      ctx.strokeStyle = C.dim;
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.arc(at.x, at.y, 11, 0, Math.PI * 2);
      ctx.stroke();
      ctx.setLineDash([]);
    }
    if (badAge >= 0 && badAge < 1600) {
      const f = badAge / 1600;
      const shake = this.reduceMotion ? 0 : Math.sin(badAge / 30) * 3 * (1 - f);
      ctx.strokeStyle = `rgba(255,93,108,${1 - f})`;
      ctx.beginPath();
      ctx.arc(at.x + shake, at.y, 13, 0, Math.PI * 2);
      ctx.moveTo(at.x - 5 + shake, at.y - 5);
      ctx.lineTo(at.x + 5 + shake, at.y + 5);
      ctx.moveTo(at.x + 5 + shake, at.y - 5);
      ctx.lineTo(at.x - 5 + shake, at.y + 5);
      ctx.stroke();
    }
    ctx.lineWidth = 1;
    ctx.font = `10px ${FONT}`;
    ctx.fillStyle = C.dim;
    ctx.textAlign = 'center';
    ctx.fillText(t('rx.seal'), at.x, at.y + 24);
    ctx.textAlign = 'left';
  }

  private writeStrip(log: ScenarioLog, rx: RxState, b: Box, T: number): void {
    const ctx = this.ctx;
    const size = Math.max(1, log.summary.newSize);
    ctx.fillStyle = C.cell;
    ctx.fillRect(b.x, b.y, b.w, b.h);
    for (const seg of rx.writes) {
      const x = b.x + (seg.off / size) * b.w;
      const w = Math.max(0.6, (seg.len / size) * b.w);
      const fresh = T - seg.t < 500;
      ctx.fillStyle = seg.kind === 'extra' ? C.extra : fresh ? '#d6beff' : C.neu;
      ctx.fillRect(x, b.y, w, b.h);
    }
    if (rx.written > 0 && rx.written < size && rx.status === 'receiving') {
      const hx = b.x + (rx.written / size) * b.w;
      ctx.fillStyle = '#fff';
      ctx.fillRect(hx - 1, b.y - 3, 2, b.h + 6);
    }
    // Completed groups that cannot be applied yet: bytes are applied strictly in order.
    let label = t('rx.write');
    let labelColor = C.dim;
    if (rx.status === 'receiving') {
      let next = 0;
      while (next < rx.G && rx.groupFedT[next] > -1e11) next++;
      let waiting = 0;
      for (let g = next + 1; g < rx.G; g++) if (rx.groupDone[g] && rx.groupFedT[g] < -1e11) waiting++;
      if (waiting > 0) {
        label = t('rx.waiting', { g: next, n: waiting });
        labelColor = C.warn;
      }
    }
    ctx.strokeStyle = rx.status === 'installed' ? C.good : C.grid;
    ctx.strokeRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1);
    ctx.font = `10px ${FONT}`;
    ctx.fillStyle = labelColor;
    this.fitText(label, b.x, b.y - 3, b.w - 40);
    ctx.fillStyle = C.dim;
    ctx.textAlign = 'right';
    ctx.fillText(`${Math.floor((100 * rx.written) / size)}%`, b.x + b.w, b.y - 3);
    ctx.textAlign = 'left';
  }

  private weather(spec: RxState['rx']['spec']): string {
    const parts: string[] = [];
    const pct = (v: number) => Math.round(v * 100);
    if (spec.loss > 0) parts.push(t('rx.loss', { x: pct(spec.loss) }));
    if (spec.burstLen > 1) parts.push(t('rx.burst', { x: spec.burstLen }));
    if (spec.corrupt > 0) parts.push(t('rx.corrupt', { x: pct(spec.corrupt) }));
    if (spec.dup > 0) parts.push(t('rx.dup', { x: pct(spec.dup) }));
    if (spec.reorder > 0) parts.push(t('rx.reorder', { x: pct(spec.reorder) }));
    if (spec.installedVersion >= 2) parts.push(t('rx.oldfw', { v: spec.installedVersion }));
    if (spec.wrongBase) parts.push(t('rx.otherbase'));
    if (spec.wrongKey) parts.push(t('rx.otherkey'));
    return parts.length ? parts.join(' · ') : t('rx.clean');
  }

  private fitText(text: string, x: number, y: number, maxW: number): void {
    const ctx = this.ctx;
    if (maxW <= 10) return;
    let s = text;
    if (ctx.measureText(s).width > maxW) {
      while (s.length > 1 && ctx.measureText(`${s}…`).width > maxW) s = s.slice(0, -1);
      s = `${s}…`;
    }
    ctx.fillText(s, x, y);
  }
}
