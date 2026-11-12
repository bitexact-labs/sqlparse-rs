# sqlparse-rs: `sqlparse.split()` in Rust, the same statements 28–36× faster

A drop-in accelerator for [sqlparse](https://github.com/andialbrecht/sqlparse) 0.6.1 (~100M downloads/month).
sqlparse is what many tools use to cut SQL scripts, dumps and migrations into statements: for example, Django's
`prepare_sql_script` runs `sqlparse.split`. On large scripts it is slow: about 6 seconds for a 2.5 MB dump.

sqlparse-rs ports sqlparse's lexer and its `StatementSplitter` to Rust and returns **exactly what sqlparse returns**:
the same statements, and the same `(ttype, value)` tokens with the very same `sqlparse.tokens` objects.

```python
import sqlparse_rs
statements = sqlparse_rs.split(sql)            # == sqlparse.split(sql)
tokens = sqlparse_rs.tokenize(sql)              # == list(sqlparse.lexer.tokenize(sql))

sqlparse_rs.install()   # or: make sqlparse itself use the Rust lexer (split, parse, format)
```

## Benchmark (Intel i5-8400T, Python 3.14, sqlparse 0.6.1)
| Task | sqlparse | sqlparse-rs | Speedup |
|---|---:|---:|---:|
| split: SQL dump, 20,000 INSERTs (2.5 MB) | 6,211 ms | 173.8 ms | **36×** |
| split: migrations with functions and $$ bodies (119 KB) | 288 ms | 10.3 ms | **28×** |
| tokenize: SQL dump (2.5 MB) | 4,928 ms | 233.8 ms | **21×** |
| format(reindent, upper) of 40 queries (11 KB), with install() | 144 ms | 115.5 ms | 1.2× |
| parse: migrations (119 KB), with install() | 936 ms | 707.9 ms | 1.3× |

Reproduce: `python bench/bench.py` (asserts equal outputs). `parse` and `format` gain little: after lexing, their time is in
sqlparse's grouping and formatting code, which stays Python. The big win is `split` and tokenizing.

## How it is the same, by construction and by test
- **No hand-translated regexes.** `gen/gen_tables.py` parses sqlparse's own `SQL_REGEX` with CPython's `re` parser and emits it
  as a small regex AST. Every character class (with `IGNORECASE`: `K` (Kelvin sign) matching `k`, `ſ` matching `s`, `ı`/`İ`...; Unicode `\w`, `\s`, `\d`, `\b`)
  is computed by running CPython's `re` engine over all 1.1M code points. Keywords, token types and `str.upper()` come from Python too.
- **Same matching order.** A backtracking matcher with sre's semantics: alternatives and repeat counts in the order sre tries them,
  lookahead, lookbehind, `$`. Long string literals and unterminated quotes are handled iteratively (no recursion, no quadratic rescans).
- **sqlparse's own test suite**, 537 tests, passes with `install()` (601 calls into the Rust core): `cd tests && PYTHONPATH=. pytest -p upstream_conftest_rs upstream`.
- **Differential fuzzing** (`tests/fuzz_vs_orig.py`): random SQL from keywords, quotes of every kind, `$tag$` bodies, nested comments,
  `BEGIN … END` blocks, `GO`, Unicode whitespace and case-folding traps, plus a "chaos" mode with random code points.
  Each case compares `tokenize` (type identity and value) and `split` with and without `strip_semicolon`.
  Final build: 150,000 SQL-like cases + 100,000 chaos cases, **0 differences** (logs: `tests/fuzz_*.log`).
- **Found while testing:** our first build was quadratic on long runs of whitespace (a lazy `\s+?` looked at the whole run).
  `tests/test_linear.py` now checks 1M-character worst cases (whitespace, literals, comments, unterminated quotes, `$$` bodies).

## Limits
- Pinned to sqlparse 0.6.1 (the generated tables are checked against it in CI); import fails on another version instead of guessing.
- Fast path only for `str` input with the default lexer configuration. `bytes`, file objects, custom keywords (`Lexer.add_keywords`)
  or a changed `SQL_REGEX` run the original sqlparse code.
- Character classes come from Python 3.14's Unicode database (16.0). On a Python with another Unicode version, non-ASCII input
  goes to sqlparse itself (exact, not faster); ASCII input stays fast.
- Strings with lone surrogates fall back to sqlparse.
- Speed per character is not uniform: identifiers cost the most (~150 ns per char), long string literals the least (~25 ns).

## Competitors
sqloxide (sqlparser-rs), sqlglot's Rust tokenizer and squonk are fast SQL *parsers* with their own grammars and outputs; none of them
reproduces sqlparse's statements or tokens, so they are not drop-in replacements. We found no accelerated sqlparse (Nov 2026).

## Build
```
pip install maturin
python -I gen/gen_tables.py > src/gen.rs   # only when updating sqlparse
maturin develop --release
```

## License
BSD-3-Clause, like sqlparse. See `LICENSE` and `NOTICE`.
