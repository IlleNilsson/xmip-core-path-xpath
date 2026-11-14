# xmip-core-path-xpath

The `xpath` path technology, a technology of
[xmip-core-path](https://github.com/IlleNilsson/xmip-core-path). It carries
`XPathLanguage`, the `PathLanguage` for the language `xpath`: an expression is
checked when configuration compiles it and built once per thread that
evaluates it (the engine's model and a compiled `XPath` are not thread-safe),
the document is read once per Message, and a rewrite reads once and
serializes once.

It is the estate's one `XPath`. The document is read by the estate's one XML
reader, `xmip-core-library-codec`'s tree, and only then built into the model
the engine evaluates over: `sxd-xpath` is written over `sxd-document`'s DOM
and no other, so the DOM is kept here as `XPath`'s evaluation model alone,
filled from codec's reader and never parsing text itself (the owner,
2026-11-13: *Do we have all code in the modules, repos that we should*).
No other crate links either. The Schematron contract evaluates its rules
through `Document`, `Expression` and `Selected`, the same expressions built
the same once a thread, and a fired node's place, `/Invoice/Line[2]`, is
said here.
A prefix resolves through the Path's `namespaces` and never through the
document, so `/o:order` reads the URI the configuration gave `o` whatever
prefix a document writes for it; a prefix the table does not bind is refused
as the expression compiles (ADR-0066, amendment 2026-11-08). Promote reads
through it, demote writes through it to produce a new Stream (ADR-0013),
route and process read.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
