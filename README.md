# OTA Updater

A broadcast over-the-air (OTA) firmware updater written in Rust, plus **OTA, explained**: an
interactive, bilingual (English / 中文) web page that teaches how it works, driven by the real
engine compiled to WebAssembly.

- **Sender**: bsdiff-style binary diff → zstd → slicing into shards → Reed-Solomon parity →
  Ed25519-signed manifest → UDP multicast broadcast in repeating rounds.
- **Receiver**: CRC32 check → Reed-Solomon reconstruction → signed per-group fingerprints →
  streaming patch into a spare slot → signed SHA-256 verification → A/B install.

## OTA, explained — the interactive page

Nine guided chapters (diff, compression, slicing, Reed-Solomon, signatures, broadcast, receiving,
install) and a broadcast sandbox where packets fly, get lost or corrupted, and devices repair and
install the update. Everything on the page is computed by the Rust engine running in your browser.
The build produces **one self-contained HTML file** that works offline.

### Prerequisites

- Rust (edition 2024, 1.85 or newer) with the WebAssembly target:
  `rustup target add wasm32-unknown-unknown`
- The `wasm-bindgen` CLI, **exactly version 0.2.128** (it must match `viz/wasm/Cargo.toml`):
  `cargo install wasm-bindgen-cli --version 0.2.128`
- Node.js 22.18+ or 23.6+ (tested with 26)
- `bash`, for the WebAssembly build script (macOS / Linux; on Windows use WSL or Git Bash)
- Optional: `wasm-opt` (binaryen) for a smaller module

### Build

```sh
git clone git@github.com:experifarm/OTA_updater.git
cd OTA_updater/viz/web
npm ci          # build tools: Vite, vite-plugin-singlefile, TypeScript
npm run wasm    # compile the Rust simulation to WebAssembly → viz/web/src/pkg/
npm run build   # bundle everything into one file        → viz/dist/ota-explained.html
```

### Open

The result is a single file of about 1 MB with the WebAssembly embedded; it needs no server and no
network. From the repository root:

```sh
open viz/dist/ota-explained.html          # macOS
xdg-open viz/dist/ota-explained.html      # Linux
start viz\dist\ota-explained.html         # Windows
```

or drag the file into a browser window. To share it, send the file.

The page opens on chapter 1. Direct links: `#learn/1` … `#learn/9` for the chapters, `#sandbox` for
the sandbox, and `#sandbox/<preset>` for a scenario (`clean`, `wifi`, `storm`, `late`, `attacker`,
`refused`) — for example `ota-explained.html#sandbox/attacker`. It has been tested in Chromium;
any current browser with WebAssembly should work.

### Develop

```sh
cd viz/web
npm run dev     # then open http://localhost:5173/ota-explained.html (the root path / is not served)
```

After changing Rust code (`src/` or `viz/wasm/`), run `npm run wasm` again. If the page shows
"WebAssembly failed to load", rebuild with `npm run wasm && npm run build`. A wasm-bindgen error
about a different "bindgen format" means the installed CLI is not version 0.2.128.

## The updater (command line)

```sh
cargo build --release
```

This builds `target/release/ota-sender` (`keygen`, `gen-images`, `diff`, `send`) and
`target/release/ota-receiver` (`listen`, `apply`). A complete local run — one sender, two lossy
receivers on loopback multicast — is in [docs/TESTING.md](docs/TESTING.md#2-native-cli-end-to-end).

## Tests

```sh
cargo test --release              # updater: unit tests, in-process and UDP end-to-end tests
cargo test -p ota_viz --release   # simulation scenarios and chapter lessons
cd viz/web && npm run smoke       # every sandbox preset through the WebAssembly module
```

All checks (formatting, clippy for every build, browser smoke test) are listed in
[docs/TESTING.md](docs/TESTING.md).

## Repository layout

```
src/          the updater library and the ota-sender / ota-receiver command-line tools
tests/        end-to-end tests (in-process and UDP multicast)
viz/wasm/     simulation and chapter lessons, compiled to WebAssembly
viz/web/      the page: TypeScript, Canvas 2D, Vite single-file build
docs/         DESIGN.md (protocol and algorithms), VISUALIZATION.md (the page), TESTING.md
```
