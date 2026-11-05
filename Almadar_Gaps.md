<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-canvas — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-GAME-RENDERER-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.

Next code: `G-ORBITAL-GAME-RENDERER-002`

- **G-ORBITAL-GAME-RENDERER-001** — `state-machine-view`'s whole-page mode (`orbital`, `activeStates`, `firedTransition`, `expandDepth`; every mounted trait as a box nested in its embedder, a background lane for non-rendering traits, listen wiring as arrows) is painted only by the web renderer (`AvlOrbitalMachine`, ELK hierarchical). The Rust mapper `src/mappers/diagram/state_machine.rs` (with `layered.rs`), which iOS, Android, TUI and Dioxus use, still draws one trait, so a native host given `orbital` draws nothing of it. Owner ruled on 2026-11-05: web first, Rust canvas after. orbital-canvas [architectural]: compound nodes in `layered.rs` plus a twin fixture of the web painter's nested layout; prevention rung (3), a renderer parity fixture for the orbital props. Root cause: one-off.
