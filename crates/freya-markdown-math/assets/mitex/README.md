The `.typ` files here are the `specs/` folder of the MiTeX Typst package
(`@preview/mitex:0.2.4`, Apache-2.0, see `LICENSE`), which defines the
functions MiTeX's converted output calls. Changed for this crate:

- `latex/standard.typ` no longer imports `@preview/xarrow` (GPL-3.0); the
  `\x...arrow` commands use Typst's own `math.stretch` instead.
- `KaTeX` is spelled with plain letters: Typst 0.15 dropped the `kai`
  symbol the original used.
