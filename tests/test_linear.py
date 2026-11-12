"""Worst cases must stay linear: 1M-char runs of whitespace, string literals, comments, unterminated quotes.
Equality with sqlparse is checked on 100k-char versions (sqlparse itself is slow on 1M)."""
import time, sqlparse, sqlparse_rs
cases = {
    "spaces": lambda n: " " * n + "select 1;",
    "string": lambda n: "select '" + "a''b" * (n // 4) + "';",
    "comment": lambda n: "-- " + "x" * n + "\nselect 1;",
    "unterminated": lambda n: "select '" + "a" * n,
    "dollar": lambda n: "do $$ " + "x;" * (n // 2) + " $$;",
    "words": lambda n: "abc " * (n // 4),
}
for name, mk in cases.items():
    small = mk(100_000)
    assert sqlparse_rs.split(small) == sqlparse.split(small), name
    big = mk(1_000_000)
    t = time.perf_counter(); sqlparse_rs.split(big); dt = time.perf_counter() - t
    print(f"{name}: 1M chars in {dt:.2f}s")
    assert dt < 5, name
print("ok")
