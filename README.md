<div align="center">

# orbital-canvas

**The shared `wgpu` renderer that paints Almadar game and diagram canvases from a JSON display list.**

[![CI](https://github.com/almadar-io/orbital-canvas/actions/workflows/ci.yml/badge.svg)](https://github.com/almadar-io/orbital-canvas/actions/workflows/ci.yml)
[![License: BUSL-1.1](https://img.shields.io/badge/license-BUSL--1.1-blue)](LICENSE)

</div>

One GPU renderer for Rust desktop, Android and iOS, reached through a small C ABI. The server or runtime sends a JSON `GameRenderPacket` (a display list); this crate turns it into draw calls. It also rasterizes SVG sprites and vector paths and shapes text (joining, bidi, per-script fonts).

## Where this fits

```mermaid
flowchart TB
  subgraph kernel["Kernel and substrate"]
    core["orbital-core<br/>IR · evaluator · kernel"]
    canvas["orbital-canvas<br/>2D/3D render primitives"]
  end
  subgraph lang["Language"]
    lolo["orbital-lolo<br/>.lolo → .orb"]
    compiler["orbital-compiler<br/>validate · codegen"]
  end
  subgraph hosts["Shells and hosts"]
    ts["orbital-shell-typescript<br/>web · node · @almadar/core"]
    rs["orbital-shell-rust<br/>desktop · tui"]
    py["orbital-shell-python"]
    ios["orbital-shell-ios"]
    android["orbital-shell-android"]
    os["orbital-shell-os"]
    wasm["orbital-wasm<br/>browser kernel"]
    ffi["orbital-ffi<br/>C ABI for mobile"]
    router["orbital-router"]
  end
  cli["orbital-cli<br/>the orb binary"]
  ws["orbital-rust<br/>workspace · cross-crate tests"]
  dist["orb<br/>npm · releases"]
  subgraph pkgs["Language packages"]
    std["almadar-std<br/>@almadar/std substrate"]
    beh["behaviors<br/>themed organisms"]
    langs["lolo-ar · lolo-sl<br/>language packs"]
  end
  subgraph apps["Apps and sites"]
    builder["builder (studio)"]
    kflow["kflow-academy"]
    nata["nata"]
    rawi["rawi"]
    site["almadar (websites)"]
  end
  core --> lolo & compiler & ts & rs & py & ios & android & os & wasm & ffi & router
  canvas --> ts & rs & ffi
  lolo --> compiler
  compiler --> cli
  ts & rs & py & ios & android & os --> cli
  cli --> dist
  ws -.pins.-> core & lolo & compiler & cli
  dist --> std
  std --> beh
  std --> langs
  beh --> builder & kflow & nata & rawi & site
  click core "https://github.com/almadar-io/orbital-core"
  click canvas "https://github.com/almadar-io/orbital-canvas"
  click lolo "https://github.com/almadar-io/orbital-lolo"
  click compiler "https://github.com/almadar-io/orbital-compiler"
  click ts "https://github.com/almadar-io/orbital-shell-typescript"
  click rs "https://github.com/almadar-io/orbital-shell-rust"
  click py "https://github.com/almadar-io/orbital-shell-python"
  click ios "https://github.com/almadar-io/orbital-shell-ios"
  click android "https://github.com/almadar-io/orbital-shell-android"
  click os "https://github.com/almadar-io/orbital-shell-os"
  click wasm "https://github.com/almadar-io/orbital-wasm"
  click ffi "https://github.com/almadar-io/orbital-ffi"
  click router "https://github.com/almadar-io/orbital-router"
  click cli "https://github.com/almadar-io/orbital-cli"
  click ws "https://github.com/almadar-io/orbital-rust"
  click dist "https://github.com/almadar-io/orb"
  click std "https://github.com/almadar-io/almadar-std"
  click beh "https://github.com/almadar-io/behaviors"
  click langs "https://github.com/almadar-io/lolo-ar"
  click builder "https://github.com/itechdom/almadar-studio"
  click kflow "https://github.com/almadar-io/almadar-kflow-academy"
  click nata "https://github.com/almadar-io/almadar-nata"
  click rawi "https://github.com/almadar-io/almadar-rawi"
  click site "https://github.com/almadar-io/almadar"
  classDef here fill:#2563eb,color:#fff,stroke:#1e40af,stroke-width:3px
  class canvas here
```

| Upstream (this repo depends on) | Downstream (depends on this repo) |
|---|---|
| [orbital-core](https://github.com/almadar-io/orbital-core): it uses the `orbital-core-base` layer | [orbital-ffi](https://github.com/almadar-io/orbital-ffi): re-exports the renderer C ABI for the mobile shells |
| | [orbital-shell-typescript](https://github.com/almadar-io/orbital-shell-typescript) and [orbital-shell-rust](https://github.com/almadar-io/orbital-shell-rust): paint canvases with it (per the architecture) |

## Quick start

The crate reads the Almadar language data from installed npm packages, so install those first. The toolchain is pinned to Rust 1.93.0 in `rust-toolchain.toml`.

```bash
npm install --no-save --no-package-lock @almadar/core@^10 @almadar/std@^16 @almadar/lolo-ar@^1 @almadar/lolo-sl@^1
cargo test
```

Two examples dump a render packet as JSON: `cargo run --example dump-canvas-packet` and `cargo run --example dump-learning-scene`.

## What's inside

| Path | Purpose |
|---|---|
| `src/packet.rs`, `src/projector.rs` | The `GameRenderPacket` display list and its projection |
| `src/renderer.rs` | `SharedDeviceRenderer`, the `wgpu` renderer |
| `src/c_api.rs` | The C ABI the native hosts call |
| `src/mappers/` | Pattern mappers: game canvas 2D, isometric, platformer, mini map, scene, diagram, learning |
| `src/svg.rs`, `src/vector.rs`, `src/atlas.rs`, `src/animate.rs`, `src/asset.rs` | SVG sprites, vector layer (paths, strokes, text), atlases, animation, asset loading |
| `src/platform/` | Android and iOS surface glue |

Cargo features (all off by default): `http` (HTTP(S) asset fetching, native only) and `system-fonts` (desktop hosts supply scripts the bundled faces lack from the installed system fonts).

## Development

- Test with `cargo test`; CI (`.github/workflows/ci.yml`) runs it on push and pull request to `main`.
- Open gaps are tracked in [`Almadar_Gaps.md`](Almadar_Gaps.md). Work on `main` only; there are no feature branches.
- Inside the monorepo this repo is the submodule `orbital-rust/crates/orbital-canvas`.

## Docs

- [Orb documentation](https://orb.almadar.io/docs): the language, the `orb` CLI and how the pieces fit

## License

[Business Source License 1.1](LICENSE): free for non-commercial use and internal evaluation; commercial production use needs a licence from Almadar FZE. Converts to Apache-2.0 on 2030-02-01.
