"""python bench/bench.py -> markdown table. sqlparse 0.6.1 vs sqlparse_rs on the same input, outputs asserted equal."""
import time, random, platform
import sqlparse, sqlparse_rs
from sqlparse import lexer

def best(f, reps):
    ts = []
    for _ in range(reps):
        t = time.perf_counter(); out = f(); ts.append(time.perf_counter() - t)
    return min(ts), out

r = random.Random(1)
dump = '\n'.join("INSERT INTO users (id, name, email, note) VALUES (%d, 'user%d', 'u%d@example.com', 'it''s; fine -- not a comment');" % (i, i, i) for i in range(20000))
migr = []
for i in range(300):
    migr.append(f"CREATE TABLE t{i} (id bigserial PRIMARY KEY, name varchar(200) NOT NULL, data jsonb, created_at timestamptz DEFAULT now());")
    migr.append(f"CREATE INDEX t{i}_name ON t{i} USING btree (lower(name));")
    migr.append(f"CREATE OR REPLACE FUNCTION f{i}(x int) RETURNS int AS $$ BEGIN IF x > 0 THEN RETURN x; END IF; RETURN -x; END; $$ LANGUAGE plpgsql;")
    migr.append(f"/* migration {i} */ ALTER TABLE t{i} ADD COLUMN flag boolean DEFAULT false; -- done")
migr = '\n'.join(migr)
query = ("SELECT u.id, u.name, count(o.id) AS orders, sum(o.total) FROM users u LEFT JOIN orders o ON o.user_id = u.id "
         "WHERE u.created_at > now() - interval '30 days' AND u.email LIKE '%@example.com' GROUP BY u.id, u.name "
         "HAVING count(o.id) > 3 ORDER BY 4 DESC NULLS LAST LIMIT 100;\n") * 40

rows = []
def row(name, f0, f1, reps, eq=lambda a, b: a == b):
    t0, a = best(f0, reps); t1, b = best(f1, reps)
    assert eq(a, b), name
    rows.append((name, t0, t1))

row(f"split: SQL dump, 20,000 INSERTs ({len(dump)/1e6:.1f} MB)", lambda: sqlparse.split(dump), lambda: sqlparse_rs.split(dump), 2)
row(f"split: migrations with functions and $$ bodies ({len(migr)/1e3:.0f} KB)", lambda: sqlparse.split(migr), lambda: sqlparse_rs.split(migr), 3)
row(f"tokenize: SQL dump ({len(dump)/1e6:.1f} MB)", lambda: list(lexer.tokenize(dump)), lambda: sqlparse_rs.tokenize(dump), 2)
fmt = lambda: sqlparse.format(query, reindent=True, keyword_case='upper')
prs = lambda: [str(s) for s in sqlparse.parse(migr)]
t0f, f0 = best(fmt, 3); t0p, p0 = best(prs, 3)
sqlparse_rs.install()
t1f, f1 = best(fmt, 3); t1p, p1 = best(prs, 3)
sqlparse_rs.uninstall()
assert f0 == f1 and p0 == p1
rows.append((f"format(reindent, upper) of 40 queries ({len(query)/1e3:.0f} KB), with install()", t0f, t1f))
rows.append((f"parse: migrations ({len(migr)/1e3:.0f} KB), with install()", t0p, t1p))

print("| Task | sqlparse | sqlparse-rs | Speedup |")
print("|---|---:|---:|---:|")
for name, t0, t1 in rows:
    print(f"| {name} | {t0*1000:,.0f} ms | {t1*1000:,.1f} ms | **{t0/t1:.0f}×** |" if t0 / t1 >= 3 else f"| {name} | {t0*1000:,.0f} ms | {t1*1000:,.1f} ms | {t0/t1:.1f}× |")
print(f"\nPython {platform.python_version()}, sqlparse {sqlparse.__version__}, best of N runs, outputs compared for equality.")
