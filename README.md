# dv

A native desktop app (Rust +
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)) for reviewing diffs locally —
above all a coding agent's work before it is committed — and for reviewing and submitting large
GitHub PRs that github.com handles badly. Windows first with first-class WSL; macOS and Linux
supported. Coding agents read and answer the review through the `dv` CLI.

dv is being rewritten. The plan is [docs/PLAN.md](docs/PLAN.md) and the visual spec is
[docs/DESIGN.md](docs/DESIGN.md); the first implementation stays in `legacy/` as reference until the
rewrite reaches parity.

## Building

Needs Rust (the toolchain is pinned in `rust-toolchain.toml`; rustup installs it), `git`, and `gh`
for GitHub features.

```
cargo run -p dv          # build and launch
script/check             # every local gate: fmt, clippy, tests, e2e scenarios
```

The first build compiles GPUI and takes several minutes; incremental builds are fast.

## Third-party assets

Bundled in the binary from `assets/`: the [Geist](https://github.com/vercel/geist-font) and
[JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) fonts (SIL Open Font License 1.1,
`assets/fonts/*/OFL.txt`) and a few [Lucide](https://lucide.dev) icons (ISC,
`assets/icons/LICENSE-LUCIDE`; `assets/icons/README.md` lists them).
