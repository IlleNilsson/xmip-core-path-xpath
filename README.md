# xmip-core-path-xpath

The `xpath` path technology, a technology of
[xmip-core-path](https://github.com/IlleNilsson/xmip-core-path). It carries
`XPathLanguage`, the `PathLanguage` for the language `xpath`: an expression is
checked when configuration compiles it and built once per thread that
evaluates it (the DOM and a compiled `XPath` are not thread-safe), the document
is parsed once per Message with the context its prefixes bind, and a rewrite
parses once and serializes once. Promote reads through it, demote writes
through it to produce a new Stream (ADR-0013), route and process read.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
