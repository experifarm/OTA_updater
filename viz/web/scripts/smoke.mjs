// Smoke test + benchmark of the real WASM simulation under Node (no browser needed).
// Usage: node viz/web/scripts/smoke.mjs   (after viz/build-wasm.sh)
import { readFileSync } from 'node:fs';
import { initSync, run_scenario, engine_version } from '../src/pkg/ota_viz.js';
import { PRESETS, buildConfig } from '../src/presets.ts';

const wasm = readFileSync(new URL('../src/pkg/ota_viz_bg.wasm', import.meta.url));
initSync({ module: wasm });
console.log(`${engine_version()} — wasm ${(wasm.length / 1024).toFixed(0)} KiB`);

let failures = 0;
for (const scale of ['toy', 'real']) {
  for (const preset of PRESETS) {
    const cfg = buildConfig(preset.id, scale);
    const t0 = performance.now();
    const json = run_scenario(JSON.stringify(cfg));
    const ms = performance.now() - t0;
    const log = JSON.parse(json);
    const statuses = log.receivers.map((r) => r.result.status);
    const ok = JSON.stringify(statuses) === JSON.stringify(preset.expect);
    if (!ok) failures++;
    const s = log.summary;
    console.log(
      `${ok ? 'ok  ' : 'FAIL'} ${scale.padEnd(4)} ${preset.id.padEnd(8)} ${ms.toFixed(0).padStart(5)} ms  ` +
        `json ${(json.length / 1024).toFixed(0).padStart(4)} KiB  patch ${s.patchSize} B (${s.compressed ? 'zstd' : 'raw'})  ` +
        `${s.groups} groups  ${log.packets.length} pkts  ${log.rounds.length} rounds  ${(log.endMs / 1000).toFixed(1)} s  ` +
        `[${statuses.join(', ')}]${ok ? '' : ` expected [${preset.expect.join(', ')}] ` + log.receivers.map((r) => r.result.reason).join(' | ')}`,
    );
  }
}
if (failures) {
  console.error(`${failures} scenario(s) did not match expectations`);
  process.exit(1);
}
