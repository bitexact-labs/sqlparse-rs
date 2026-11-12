"""Fuzz: sqlparse_rs.tokenize / split must equal sqlparse's own lexer / split exactly.
usage: fuzz_vs_orig.py SEED N"""
import sys, random
import sqlparse, sqlparse_rs
from sqlparse import lexer

seed, N = int(sys.argv[1]), int(sys.argv[2])
r = random.Random(seed)
WORDS = ("select from where insert into values update set delete create or replace function procedure begin end "
         "if loop while for do case when then else declare transaction work commit rollback go join left outer "
         "inner cross natural group by order asc desc nulls first last union all not null like ilike regexp "
         "exists primary key handler double precision lateral view explode at time zone returns language "
         "plpgsql int varchar table index as in using on and is distinct limit offset with recursive").split()
PIECES = ["'", "''", '"', '""', "`", "``", "´", "\\'", '\\"', "$$", "$tag$", "$a1$", "$_$", "/*", "*/", "/*+", "--", "-- +",
          "# ", "#", "##", "@", ":=", "::", "*", "?", "%s", "%(name)s", ":name", "$1", "?x", "\\copy", ".", ",", ";", "(", ")",
          "[", "]", "[x]", "a[1]", "0x1F", "-0xAb", "1.5e3", "1E-2", "-3", ".5", "5.", "1.2.3", "12abc", "1_000", "x.y", "a . b",
          " .5", "->", "->>", "#>", "#>>", "@>", "<@", "?|", "?&", "#-", "<=", "<>", "!=", "~", "+", "-", "/", "%", "^", "&", "|",
          " ", "  ", "\t", "\n", "\r\n", "\r", "　", " ", "\x1c", " ", "\x0b", "\x0c",
          "K", "ſ", "ı", "İ", "ß", "ﬁ", "é", "Ö", "Ü", "ÿ", "µ", "ǅ", "日本", "٣", "²", "Ⅻ", "́", "_", "ä_x", "x1", "GO 2", "GO",
          "END IF", "END  LOOP", "end\tcase", "CREATE OR　REPLACE", "AT TIME ZONE 'UTC'", "WITH' TIME ZONE 'x'", "e", "E"]

def gen():
    parts = []
    for _ in range(r.randint(1, 60)):
        k = r.random()
        if k < 0.45:
            w = r.choice(WORDS)
            w = w.upper() if r.random() < 0.4 else (w.capitalize() if r.random() < 0.3 else w)
            parts.append(w)
        elif k < 0.85:
            parts.append(r.choice(PIECES))
        elif k < 0.93:
            parts.append(str(r.randint(-50, 10**6)))
        else:
            parts.append(''.join(chr(r.choice([r.randint(32, 126), r.randint(0x80, 0x24F), r.randint(0x370, 0x3FF), r.randint(0x2000, 0x2BFF), r.randint(0x10000, 0x1F9FF)])) for _ in range(r.randint(1, 4))))
        parts.append(r.choice(["", " ", " ", "\n", ""]))
    return ''.join(parts)

CHAOS = len(sys.argv) > 3 and sys.argv[3] == "chaos"
ALPH = list("'\"`´$/*+-#@:;.,()[]?%_\\eExX0123456789 \t\n\r") + ["\u3000", "\x1c", "\x85", "\u2028", "K", "ſ", "ı", "İ", "ß", "é", "٣", "²", "\u0301"]
def chaos():
    n = r.randint(0, 80)
    return ''.join(r.choice(ALPH) if r.random() < 0.7 else chr(r.choice([r.randint(0x20, 0x7e), r.randint(0x80, 0x2fff), r.randint(0xe000, 0x10ffff)])) for _ in range(n))

# the Rust core carries Python 3.14's Unicode tables; on another Unicode version the public API sends
# non-ASCII input to sqlparse itself, so there we check the public API (what users call), not the bare core
if sqlparse_rs._EXACT_UNICODE:
    core_tokenize = lambda s: sqlparse_rs._core.tokenize(s, sqlparse_rs._TYPES)
    core_split = lambda s, strip: sqlparse_rs._core.split(s, strip)
else:
    core_tokenize = sqlparse_rs.tokenize
    core_split = lambda s, strip: sqlparse_rs.split(s, strip_semicolon=strip)
    print("Unicode", sqlparse_rs._ud.unidata_version, "!= core", sqlparse_rs._core.UNIDATA_VERSION, "-> checking the public API")

fails = 0
for case in range(N):
    s = chaos() if CHAOS else gen()
    want = list(lexer.tokenize(s))
    try:
        got = core_tokenize(s)
    except Exception as e:
        got = repr(e)
    ok = want == got and all(a[0] is b[0] for a, b in zip(want, got))
    if ok:
        for strip in (False, True):
            if sqlparse.split(s, strip_semicolon=strip) != core_split(s, strip):
                ok = False
    if not ok:
        fails += 1
        if fails <= 3:
            print("MISMATCH", repr(s)[:500])
            if isinstance(got, str): print(" got", got)
            else:
                for i, (a, b) in enumerate(zip(want, got)):
                    if a != b or a[0] is not b[0]:
                        print(" at", i, a, b); break
                else: print(" lens", len(want), len(got), "split:", sqlparse.split(s)[:3], core_split(s, False)[:3])
print(f"seed={seed} cases={N} mismatches={fails}")
sys.exit(1 if fails else 0)
