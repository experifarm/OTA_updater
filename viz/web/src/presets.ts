// Scenario presets shared by the app and the Node smoke test (type-erasable TS only, no enums).

export type Scale = 'toy' | 'real';

export interface ReceiverSpec {
  name: string;
  loss?: number;
  burstLen?: number;
  corrupt?: number;
  dup?: number;
  reorder?: number;
  seed?: number;
  joinMs?: number;
  installedVersion?: number;
  wrongBase?: boolean;
  wrongKey?: boolean;
}

export interface ScenarioConfig {
  scale: Scale;
  imageSize: number;
  seed: number;
  shardSize: number;
  k: number;
  m: number;
  interleave: number;
  manifestEvery: number;
  maxRounds: number;
  ratePps: number;
  latencyMs: number;
  roundGapMs: number;
  compress: boolean;
  fwVersion: number;
  attacker: boolean;
  receivers: ReceiverSpec[];
}

export type PresetId = 'clean' | 'wifi' | 'storm' | 'late' | 'attacker' | 'refused';

export interface Preset {
  id: PresetId;
  attacker: boolean;
  interleave?: number;
  receivers: ReceiverSpec[];
  /** Expected final status per receiver (used by the smoke test). */
  expect: string[];
}

export const SCALES: Record<Scale, Omit<ScenarioConfig, 'attacker' | 'receivers'>> = {
  toy: {
    scale: 'toy',
    imageSize: 0,
    seed: 1,
    shardSize: 16,
    k: 4,
    m: 2,
    interleave: 2,
    manifestEvery: 24,
    maxRounds: 6,
    ratePps: 8,
    latencyMs: 1100,
    roundGapMs: 900,
    compress: false,
    fwVersion: 2,
  },
  real: {
    scale: 'real',
    imageSize: 1 << 20,
    seed: 1,
    shardSize: 512,
    k: 8,
    m: 4,
    interleave: 4,
    manifestEvery: 48,
    maxRounds: 6,
    ratePps: 24,
    latencyMs: 1100,
    roundGapMs: 900,
    compress: true,
    fwVersion: 2,
  },
};

export const PRESETS: Preset[] = [
  {
    id: 'clean',
    attacker: false,
    receivers: [
      { name: 'Car', seed: 1 },
      { name: 'Watch', seed: 2 },
    ],
    expect: ['installed', 'installed'],
  },
  {
    id: 'wifi',
    attacker: false,
    receivers: [
      { name: 'Car', loss: 0.1, seed: 3 },
      { name: 'Phone', loss: 0.15, corrupt: 0.04, dup: 0.04, reorder: 0.06, seed: 4 },
      { name: 'Watch', loss: 0.05, seed: 5 },
    ],
    expect: ['installed', 'installed', 'installed'],
  },
  {
    id: 'storm',
    attacker: false,
    interleave: 4,
    receivers: [
      { name: 'Car', loss: 0.25, burstLen: 6, seed: 6 },
      { name: 'Phone', loss: 0.1, seed: 7 },
    ],
    expect: ['installed', 'installed'],
  },
  {
    id: 'late',
    attacker: false,
    receivers: [
      { name: 'Car', seed: 8 },
      { name: 'Phone', joinMs: 5000, seed: 9 },
    ],
    expect: ['installed', 'installed'],
  },
  {
    id: 'attacker',
    attacker: true,
    receivers: [
      { name: 'Car', loss: 0.05, seed: 10 },
      { name: 'Phone', loss: 0.05, seed: 11 },
    ],
    expect: ['installed', 'installed'],
  },
  {
    id: 'refused',
    attacker: false,
    receivers: [
      { name: 'Car', seed: 12 },
      { name: 'Old car', installedVersion: 2, seed: 13 },
      { name: 'Other model', wrongBase: true, seed: 14 },
      { name: 'Fake key', wrongKey: true, seed: 15 },
    ],
    expect: ['installed', 'rejected', 'rejected', 'failed'],
  },
];

export function buildConfig(id: PresetId, scale: Scale, overrides: Partial<ScenarioConfig> = {}): ScenarioConfig {
  const preset = PRESETS.find((p) => p.id === id) ?? PRESETS[0];
  const base = SCALES[scale];
  return {
    ...base,
    interleave: preset.interleave ?? base.interleave,
    attacker: preset.attacker,
    receivers: preset.receivers.map((r) => ({ ...r })),
    ...overrides,
  };
}
