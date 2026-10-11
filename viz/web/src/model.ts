// Replays a receiver's event log into renderable state at time T (incremental forward, rebuild on seek back).

import type { Delivery, RxLog, ScenarioLog } from './types.ts';

export const CELL_EMPTY = 0;
export const CELL_BUFFERED = 1;
export const CELL_STORED = 2;
export const CELL_REBUILT = 3;
export const CELL_PAD = 4;

export type RxStatus = 'asleep' | 'listening' | 'receiving' | 'installed' | 'rejected' | 'failed';

export interface WriteSeg {
  off: number;
  len: number;
  kind: 'diff' | 'extra';
  t: number;
}

const NEVER = -1e12;

export class RxState {
  readonly G: number;
  readonly n: number;
  readonly k: number;
  cells: Uint8Array;
  cellT: Float64Array;
  groupDone: Uint8Array;
  groupDoneT: Float64Array;
  groupFailT: Float64Array;
  groupFedT: Float64Array;
  groupRedundantT: Float64Array;
  manifest: 'waiting' | 'ok' = 'waiting';
  manifestT = NEVER;
  badSigT = NEVER;
  status: RxStatus = 'listening';
  statusT = NEVER;
  reason: string | null = null;
  completed = 0;
  written = 0;
  writes: WriteSeg[] = [];
  crcT = NEVER;
  ignoredT = NEVER;
  private cursor = 0;
  private time = NEVER;

  constructor(
    private readonly log: ScenarioLog,
    readonly rx: RxLog,
  ) {
    this.G = log.summary.groups;
    this.k = log.summary.k;
    this.n = log.summary.k + log.summary.m;
    this.cells = new Uint8Array(this.G * this.n);
    this.cellT = new Float64Array(this.G * this.n).fill(NEVER);
    this.groupDone = new Uint8Array(this.G);
    this.groupDoneT = new Float64Array(this.G).fill(NEVER);
    this.groupFailT = new Float64Array(this.G).fill(NEVER);
    this.groupFedT = new Float64Array(this.G).fill(NEVER);
    this.groupRedundantT = new Float64Array(this.G).fill(NEVER);
    this.reset();
  }

  private reset(): void {
    this.cells.fill(CELL_EMPTY);
    this.cellT.fill(NEVER);
    this.groupDone.fill(0);
    this.groupDoneT.fill(NEVER);
    this.groupFailT.fill(NEVER);
    this.groupFedT.fill(NEVER);
    this.groupRedundantT.fill(NEVER);
    this.manifest = 'waiting';
    this.manifestT = NEVER;
    this.badSigT = NEVER;
    this.status = this.rx.spec.joinMs > 0 ? 'asleep' : 'listening';
    this.statusT = NEVER;
    this.reason = null;
    this.completed = 0;
    this.written = 0;
    this.writes = [];
    this.crcT = NEVER;
    this.ignoredT = NEVER;
    this.cursor = 0;
    this.time = NEVER;
  }

  private set(g: number, i: number, v: number, t: number): void {
    const idx = g * this.n + i;
    this.cells[idx] = v;
    this.cellT[idx] = t;
  }

  /** Bring the state to time T. */
  advance(T: number): void {
    if (T < this.time) this.reset();
    this.time = T;
    if (this.status === 'asleep' && T >= this.rx.spec.joinMs) this.status = 'listening';
    const evs = this.rx.events;
    while (this.cursor < evs.length && evs[this.cursor].t <= T) {
      const e = evs[this.cursor++];
      switch (e.type) {
        case 'crc_bad':
          this.crcT = e.t;
          break;
        case 'ignored':
          this.ignoredT = e.t;
          break;
        case 'manifest_bad_sig':
          this.badSigT = e.t;
          break;
        case 'manifest_ok': {
          this.manifest = 'ok';
          this.manifestT = e.t;
          this.status = 'receiving';
          // Buffered shards of other sessions are dropped; those of this session are re-stored below.
          for (let x = 0; x < this.cells.length; x++) if (this.cells[x] === CELL_BUFFERED) this.cells[x] = CELL_EMPTY;
          this.log.groups.forEach((grp, g) => {
            for (let i = grp.real; i < this.k; i++) this.set(g, i, CELL_PAD, e.t);
          });
          break;
        }
        case 'buffered':
          if (e.g < this.G && e.i < this.n) this.set(e.g, e.i, CELL_BUFFERED, e.t);
          break;
        case 'stored':
          this.set(e.g, e.i, CELL_STORED, e.t);
          break;
        case 'redundant':
          this.groupRedundantT[e.g] = e.t;
          break;
        case 'decoded':
          if (e.ok) {
            for (const i of e.missing) this.set(e.g, i, CELL_REBUILT, e.t);
            this.groupDone[e.g] = 1;
            this.groupDoneT[e.g] = e.t;
            this.completed++;
          } else {
            this.groupFailT[e.g] = e.t;
            for (let i = 0; i < this.n; i++) {
              const idx = e.g * this.n + i;
              if (this.cells[idx] !== CELL_PAD) this.set(e.g, i, CELL_EMPTY, e.t);
            }
          }
          break;
        case 'released':
          this.groupFedT[e.g] = e.t;
          break;
        case 'write':
          this.writes.push({ off: e.off, len: e.len, kind: e.kind, t: e.t });
          this.written = Math.max(this.written, e.off + e.len);
          break;
        case 'installed':
          this.status = 'installed';
          this.statusT = e.t;
          break;
        case 'rejected':
          this.status = 'rejected';
          this.statusT = e.t;
          this.reason = e.reason;
          break;
        case 'failed':
          this.status = 'failed';
          this.statusT = e.t;
          this.reason = e.reason;
          break;
        default:
          break;
      }
    }
  }
}

/** Deliveries indexed by packet, per receiver: `byPacket[r][p]`. */
export function indexDeliveries(log: ScenarioLog): Delivery[][][] {
  return log.receivers.map((rx) => {
    const by: Delivery[][] = Array.from({ length: log.packets.length }, () => []);
    for (const d of rx.deliveries) by[d.p]?.push(d);
    return by;
  });
}

/** Index of the last packet sent at or before T (−1 if none). */
export function lastPacketAt(log: ScenarioLog, T: number): number {
  let lo = 0;
  let hi = log.packets.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (log.packets[mid].t <= T) {
      ans = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return ans;
}
