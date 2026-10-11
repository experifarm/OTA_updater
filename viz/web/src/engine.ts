// The real Rust engine (WASM): initialisation, chapter lessons and scenarios.

import wasmBase64 from 'virtual:ota-wasm';
import init, { engine_version, run_lesson, run_scenario } from './pkg/ota_viz.js';
import { buildConfig, type PresetId, type Scale, type ScenarioConfig } from './presets.ts';
import type { ScenarioLog } from './types.ts';

let ready: Promise<void> | null = null;

/** Instantiate the module from the embedded bytes (no fetch). */
export function initEngine(): Promise<void> {
  if (!ready) {
    const bytes = Uint8Array.from(atob(wasmBase64), (c) => c.charCodeAt(0));
    ready = init({ module_or_path: bytes }).then(() => undefined);
  }
  return ready;
}

export function engineVersion(): string {
  return engine_version();
}

/** Run a chapter lesson (see viz/wasm/src/lessons.rs). */
export function lesson<T>(name: string, args: object = {}): T {
  return JSON.parse(run_lesson(name, JSON.stringify(args))) as T;
}

export function scenario(cfg: ScenarioConfig): ScenarioLog {
  return JSON.parse(run_scenario(JSON.stringify(cfg))) as ScenarioLog;
}

const cache = new Map<string, ScenarioLog>();

/** A preset scenario, computed once. */
export function presetLog(id: PresetId, scale: Scale): ScenarioLog {
  const key = `${id}/${scale}`;
  let log = cache.get(key);
  if (!log) {
    log = scenario(buildConfig(id, scale));
    cache.set(key, log);
  }
  return log;
}
