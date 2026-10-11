// Shape of the event log produced by the Rust simulation (viz/wasm/src/sim.rs).

import type { ReceiverSpec, Scale } from './presets.ts';

export interface Summary {
  scale: Scale;
  oldSize: number;
  newSize: number;
  rawPatch: number;
  patchSize: number;
  compressed: boolean;
  records: number;
  diffBytes: number;
  extraBytes: number;
  groups: number;
  shardSize: number;
  k: number;
  m: number;
  packetsPerRound: number;
  intervalMs: number;
  latencyMs: number;
  manifestLen: number;
  manifestFrags: number;
}

export interface ManifestCard {
  session: string;
  fwVersion: number;
  oldSha: string;
  newSha: string;
  patchSha: string;
  publicKey: string;
  signature: string;
  groupHashes: string[];
}

/** kind: 0 manifest fragment, 1 shard, 2 attacker manifest, 3 attacker shard. */
export interface PacketRec {
  t: number;
  kind: 0 | 1 | 2 | 3;
  g: number;
  i: number;
  r: number;
  src: 0 | 1;
}

export type Fate = 'ok' | 'lost' | 'corrupt' | 'dup' | 'late' | 'off';

export interface Delivery {
  p: number;
  t: number;
  f: Fate;
}

export type RxEvent =
  | { type: 'crc_bad'; t: number; p: number }
  | { type: 'bad'; t: number; p: number; reason: string }
  | { type: 'ignored'; t: number; p: number }
  | { type: 'manifest_frag'; t: number; p: number; index: number; count: number }
  | { type: 'manifest_bad_sig'; t: number; p: number }
  | { type: 'manifest_ok'; t: number; p: number }
  | { type: 'rejected'; t: number; p: number; reason: string }
  | { type: 'buffered'; t: number; p: number; g: number; i: number }
  | { type: 'stored'; t: number; p: number; g: number; i: number }
  | { type: 'redundant'; t: number; p: number; g: number; i: number }
  | { type: 'decoded'; t: number; g: number; used: number[]; missing: number[]; ok: boolean }
  | { type: 'released'; t: number; g: number; len: number }
  | { type: 'write'; t: number; off: number; len: number; kind: 'diff' | 'extra'; old?: number }
  | { type: 'verified'; t: number; ok: boolean; sha: string }
  | { type: 'installed'; t: number }
  | { type: 'failed'; t: number; reason: string };

export interface RxStats {
  seen: number;
  dropped: number;
  corrupted: number;
  duplicated: number;
  reordered: number;
  badPackets: number;
  redundantShards: number;
  shardsRecovered: number;
  groupsRepaired: number;
  hashFailures: number;
  signatureFailures: number;
  maxPendingGroups: number;
  written: number;
}

export interface RxLog {
  spec: Required<ReceiverSpec>;
  deliveries: Delivery[];
  events: RxEvent[];
  /** For every event, the index of the delivery whose processing produced it (4294967295 if none). */
  eventDelivery: number[];
  result: { status: 'installed' | 'rejected' | 'failed'; reason: string | null; t: number; stats: RxStats };
}

export interface ScenarioLog {
  summary: Summary;
  manifest: ManifestCard;
  toy: { old: string; new: string; class: number[] } | null;
  heat: { buckets: number; changed: number[]; extra: number[] };
  records: [number, number, number, number][];
  groups: { len: number; real: number }[];
  packets: PacketRec[];
  rounds: number[];
  endMs: number;
  receivers: RxLog[];
}
