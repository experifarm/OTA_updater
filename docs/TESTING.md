# Testing and pushing

## Prerequisites

| Tool | Needed for | Notes |
|---|---|---|
| Rust toolchain (edition 2024, ≥ 1.85) | everything | tested with 1.100 nightly on aarch64-apple-darwin |
| C compiler | native build (`zstd-sys`) | Xcode command-line tools on macOS |
| `rustup target add wasm32-unknown-unknown` | WASM build | |
| `wasm-bindgen` CLI **0.2.128** | WASM build | must match the version pinned in `viz/wasm/Cargo.toml`: `cargo install wasm-bindgen-cli --version 0.2.128` |
| `wasm-opt` (binaryen) | optional | used by `viz/build-wasm.sh` when present |
| Node.js with built-in TypeScript type stripping (22.18+ / 23.6+) | visualization | tested with 26.10 |
| `playwright-core` + Chromium | optional browser test | see §3 |

## 1. Rust: core and simulation

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings                          # native (default features)
cargo clippy --all-targets --no-default-features -- -D warnings    # pure Rust
cargo clippy --lib --target wasm32-unknown-unknown --no-default-features -- -D warnings
cargo clippy -p ota_viz --all-targets -- -D warnings               # simulation crate

cargo test --release                        # core: unit tests, in-process e2e, UDP multicast e2e
cargo test --release --no-default-features  # core again with pure-Rust zstd (ruzstd)
cargo test -p ota_viz --release             # simulation scenarios + chapter lessons
```

Expected at commit `77c8bf5`:

| Command | Tests |
|---|---|
| `cargo test --release` | 59 unit + 13 in-process e2e + 1 UDP multicast e2e |
| `cargo test --release --no-default-features` | 57 unit + 13 in-process e2e (no sockets, no key files) |
| `cargo test -p ota_viz --release` | 10 lessons + 7 scenarios |

`tests/e2e_udp.rs` joins multicast group `239.255.77.9` on `127.0.0.1` with two receivers, so it
needs a loopback interface with multicast (macOS `lo0` has it) and fails where sockets are blocked.
On Linux the loopback interface may need `ip link set lo multicast on` (not verified here).

## 2. Native CLI, end to end

```sh
cargo build --release
B=target/release; D=target/demo/manual; mkdir -p $D
$B/ota-sender keygen --out-dir $D/keys
$B/ota-sender gen-images --out-dir $D/images --size 4194304
$B/ota-receiver listen --name rxA --old $D/images/old.img --out $D/rxA.img \
    --pubkey $D/keys/ota_signing.pub --installed-version 1 --loss 0.05 &
$B/ota-receiver listen --name rxB --old $D/images/old.img --out $D/rxB.img \
    --pubkey $D/keys/ota_signing.pub --installed-version 1 --loss 0.15 --burst-len 4 \
    --corrupt 0.01 --dup 0.01 --reorder 0.02 &
sleep 0.5
$B/ota-sender send --old $D/images/old.img --new $D/images/new.img \
    --key $D/keys/ota_signing.key --fw-version 2
wait
cmp $D/rxA.img $D/images/new.img && cmp $D/rxB.img $D/images/new.img && echo "both installs identical"
```

Keep demo output under `target/` (or another ignored path): `keys/`, `images/`, `*.img` and
`*.part` are ignored by `.gitignore`, but generated files are best kept out of the source tree.

## 3. Visualization

```sh
cd viz/web
npm ci                 # vite, vite-plugin-singlefile, typescript (from package-lock.json)
npm run wasm           # viz/build-wasm.sh: cargo → wasm32, wasm-bindgen (+ wasm-opt) → src/pkg
npm run smoke          # every preset × scale through the real WASM module under Node (12 runs)
npm run typecheck
npm run build          # → viz/dist/ota-explained.html (single self-contained file)
```

Open `viz/dist/ota-explained.html` in a browser (no server needed); `npm run dev` starts a live
development server.

Browser smoke test: plays every sandbox preset at both scales, renders all nine chapters in English
and Chinese, exercises the key interactions, fails on any console error, and saves screenshots to
`target/viz-shots/` (or the directory given as the first argument). Expected: 31 `ok` lines.

```sh
# from the repository root, after `npm run build`
PLAYWRIGHT_CORE=/absolute/path/to/playwright-core/index.mjs node viz/web/scripts/browser-smoke.mjs
```

`PLAYWRIGHT_CORE` points at any existing `playwright-core` installation whose Chromium is already
downloaded (that is how it was run for this commit). Without the variable the script imports
`playwright-core` from `viz/web/node_modules`; installing it there would be
`npm install --no-save playwright-core && npx playwright-core install chromium` (not verified here).

## 4. Everything at once

```sh
cargo fmt --all --check \
  && cargo clippy --all-targets -- -D warnings \
  && cargo clippy --all-targets --no-default-features -- -D warnings \
  && cargo clippy --lib --target wasm32-unknown-unknown --no-default-features -- -D warnings \
  && cargo clippy -p ota_viz --all-targets -- -D warnings \
  && cargo test --release \
  && cargo test --release --no-default-features \
  && cargo test -p ota_viz --release \
  && (cd viz/web && npm ci && npm run wasm && npm run smoke && npm run typecheck && npm run build)
```

## 5. Pushing

The repository has no remote yet (`git remote -v` is empty) and history is on `main`.

Before pushing:

```sh
git status                                   # clean working tree
git ls-files keys images viz/dist viz/web/node_modules viz/web/src/pkg | wc -l   # must print 0
git log --oneline                            # the commits you intend to publish
```

Create a new GitHub repository and push in one step (GitHub CLI, signed in):

```sh
gh repo create OTA_Updater --private --source . --remote origin --push   # or --public
```

Or push to an existing, empty repository:

```sh
git remote add origin git@github.com:<owner>/OTA_Updater.git
git push -u origin main
```

Verify:

```sh
git remote -v
git status -sb                  # "## main...origin/main" with nothing ahead or behind
git log --oneline origin/main -3
```
