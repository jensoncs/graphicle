# Request URL propagation acceptance measurement

The second extraction change follows immutable JS/TS URL bindings and small
same-file request helpers. It extends the existing route pass and preserves the
workspace linker's method, origin and ambiguity checks.

| Metric | Before | After |
| --- | ---: | ---: |
| Expected cross-repository links found | 1 / 11 | 11 / 11 |
| Recall on these positive cases | 9.1% | 100% |
| Unexpected links in abstention controls | 0 | 0 |
| Previous method-extraction acceptance set | 12 / 12 | 12 / 12 |

The [29-case acceptance test](../../crates/graphicle-memory/tests/http_url_extraction.rs)
has 11 positive cases and 18 abstention controls. These are synthetic development
cases, **not a held-out or production recall estimate**. Runtime performance and
deployment wiring were not measured. Some controls intentionally require
abstention on valid but unsupported syntax, such as escaped strings or mutable
bindings; they do not assert that a runtime request is impossible.

The baseline includes the preceding HTTP-method fix (AST schema 18), which was
still uncommitted. The [per-case receipt](http-url-extraction.json) records that
baseline's source hashes as well as the Git HEAD; HEAD alone does not reproduce
the baseline. The same test and expected results run before and after the URL
change. The new schema is 19, causing normal projection refreshes to rebuild
request facts and links.

## Supported scope

- Same-file `const` strings and aliases, including module constants, string
  concatenation, templates and parentheses. Bindings use lexical scope; an
  initializer must precede the reference in source. Escapes are not decoded.
- Named functions and constant arrow/function-expression helpers with exactly
  one returned request (optionally awaited). A helper has zero or one simple
  parameter, which may carry a TypeScript annotation; arguments must be statically
  known strings. The request method follows the existing literal-option rules.
- Helpers directly call supported `fetch`, Axios or named-client verb forms.
  Only one helper hop is expanded. Each call site gets its own substituted URL
  and source line; arguments from different calls are never pooled.

Mutable bindings, assignment/update/destructuring writes to helpers, shadows,
dynamic initializers or arguments, multi-statement/conditional/recursive
helpers, imported helpers, member-valued URLs and unknown methods are not newly
resolved. Client identity still follows existing naming conventions; global
rebinding, interceptors, runtime defaults and control-flow feasibility are not
proved. Existing partial-path handling of direct literals is preserved.

Each propagation attempt has a depth limit of 16, 64 expression steps and a
4,096-byte resulting string limit. Exceeding a limit yields no propagated URL.
No code is executed and no additional files or services are read.

## Verification and reproduction

The acceptance test passes actual TypeScript through the route extractor and
the real workspace HTTP exporter/linker, using independently supplied expected
targets. Only the selected caller declaration is seeded: the score isolates its
HTTP dependencies and does not measure symbol discovery or all helper-body
edges. Separately, the incremental extraction regression runs the full engine
without seeded declarations: changing a URL or helper prefix replaces the old
link, and making the URL dynamic removes it.

```sh
cd crates/graphicle-memory
cargo test --locked --no-default-features --test http_url_extraction --test http_method_extraction -- --nocapture
cd ../graphicle-graph
cargo test --locked --no-default-features --lib extractors::routes::
cargo test --locked --no-default-features --test routes_incremental
```

The URL receipt is emitted on the `HTTP_URL_EVALUATION=` line. Runs use Rust
1.94.1 on macOS ARM64 with locked dependencies; the memory build uses the
JavaScript/TypeScript/TSX grammar pack languages. Native regressions cover
call-site separation, lexical shadows, helper mutation and evaluation bounds.

Full repository CI remains unverified. Repository-wide formatting and strict
Clippy have pre-existing findings in unchanged files, documented in the previous
measurement. These focused checks do not replace those gates.
