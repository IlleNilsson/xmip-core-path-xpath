# xmip-core-path-xpath

The `xpath` path technology, a technology of
[xmip-core-path](https://github.com/IlleNilsson/xmip-core-path). It carries the
`PathEngine` for the language `xpath`, and the structure reader and writer that
give the engine content to address. Promote reads through it, demote writes
through it to produce a new Stream (ADR-0013), route and process read.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
