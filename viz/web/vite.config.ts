import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

const WASM = fileURLToPath(new URL('./src/pkg/ota_viz_bg.wasm', import.meta.url));

/** `import wasmBase64 from 'virtual:ota-wasm'` → the simulation module as a base64 string. Instantiating
 * from bytes avoids fetch(), which keeps the single HTML file working offline and in sandboxed previews. */
function wasmBase64(): Plugin {
  const id = 'virtual:ota-wasm';
  return {
    name: 'ota-wasm-base64',
    resolveId(source) {
      return source === id ? `\0${id}` : undefined;
    },
    load(resolved) {
      if (resolved !== `\0${id}`) return undefined;
      this.addWatchFile(WASM);
      return `export default ${JSON.stringify(readFileSync(WASM).toString('base64'))};`;
    },
  };
}

export default defineConfig({
  base: './',
  plugins: [wasmBase64(), viteSingleFile()],
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    target: 'es2022',
    reportCompressedSize: false,
    rollupOptions: { input: 'ota-explained.html' },
  },
});
