| Task | sqlparse | sqlparse-rs | Speedup |
|---|---:|---:|---:|
| split: SQL dump, 20,000 INSERTs (2.5 MB) | 6,211 ms | 173.8 ms | **36×** |
| split: migrations with functions and $$ bodies (119 KB) | 288 ms | 10.3 ms | **28×** |
| tokenize: SQL dump (2.5 MB) | 4,928 ms | 233.8 ms | **21×** |
| format(reindent, upper) of 40 queries (11 KB), with install() | 144 ms | 115.5 ms | 1.2× |
| parse: migrations (119 KB), with install() | 936 ms | 707.9 ms | 1.3× |

Python 3.14.4, sqlparse 0.6.1, best of N runs, outputs compared for equality.
