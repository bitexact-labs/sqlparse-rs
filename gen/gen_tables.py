"""Generate src/gen.rs from the installed sqlparse: lexer rules as a small regex AST,
character sets brute-forced with Python's own `re` over every code point, keyword table,
token types and their splitter flags, str.upper()/str.isspace() tables.

Run with the target sqlparse importable:  python -I gen/gen_tables.py > src/gen.rs
Everything that decides a match comes from CPython itself, so IGNORECASE folding
(K, ſ, ı, İ ...), Unicode \\w \\s \\d and \\b are the same by construction.
"""
import re
import sys
from re import _parser, _compiler, _constants as C

import sqlparse
from sqlparse import keywords, tokens
from sqlparse.lexer import Lexer

FLAGS = re.IGNORECASE | re.UNICODE
CODEPOINTS = [c for c in range(0x110000) if not (0xD800 <= c <= 0xDFFF)]
ALL = ''.join(map(chr, CODEPOINTS))


def brute(pattern_obj):
    """code points matched by a pattern that consumes exactly one char"""
    out = []
    for m in pattern_obj.finditer(ALL):
        assert m.end() - m.start() == 1
        out.append(CODEPOINTS[m.start()])
    return out


def ranges(cps):
    cps = sorted(set(cps))
    rs = []
    for c in cps:
        if rs and rs[-1][1] == c - 1:
            rs[-1][1] = c
        else:
            rs.append([c, c])
    return rs


SETS = []          # list of range lists
SET_IDS = {}       # key -> id
SET_CPS = []       # python sets (for first-set computation)


def set_id_for(key, cps):
    if key not in SET_IDS:
        SET_IDS[key] = len(SETS)
        SETS.append(ranges(cps))
        SET_CPS.append(frozenset(cps))
    return SET_IDS[key]


def node_set(node, flags):
    key = (repr(node), flags)
    if key in SET_IDS:
        return SET_IDS[key]
    state = _parser.State()
    state.flags = flags
    sp = _parser.SubPattern(state, [node])
    pat = _compiler.compile(sp, flags)
    return set_id_for(key, brute(pat))


def regex_set(rx, flags):
    key = ('rx', rx, flags)
    if key in SET_IDS:
        return SET_IDS[key]
    return set_id_for(key, brute(re.compile(rx, flags)))


SINGLE = {'LITERAL', 'NOT_LITERAL', 'IN', 'ANY'}


def emit(data, flags):
    """parsed pattern -> Rust constructor expression (a Seq)"""
    return 'seq(vec![' + ', '.join(emit_node(op, av, flags) for op, av in data) + '])'


def emit_node(op, av, flags):
    name = str(op)
    if name in SINGLE:
        return f'set({node_set((op, av), flags)})'
    if name in ('MAX_REPEAT', 'MIN_REPEAT'):
        lo, hi, body = av
        hi = 'INF' if hi == C.MAXREPEAT else str(hi)
        greedy = 'true' if name == 'MAX_REPEAT' else 'false'
        return f'rep({lo}, {hi}, {greedy}, {emit(body, flags)})'
    if name == 'SUBPATTERN':
        group, add, dele, body = av
        assert add == 0 and dele == 0
        return emit(body, flags)
    if name == 'BRANCH':
        return 'alt(vec![' + ', '.join(emit(b, flags) for b in av[1]) + '])'
    if name in ('ASSERT', 'ASSERT_NOT'):
        direction, body = av
        neg = 'true' if name == 'ASSERT_NOT' else 'false'
        if direction < 0:
            assert len(body) == 1 and str(body[0][0]) in SINGLE, body
            return f'behind({neg}, {node_set(body[0], flags)})'
        return f'ahead({neg}, {emit(body, flags)})'
    if name == 'AT':
        if str(av) == 'AT_BOUNDARY':
            return 'boundary()'
        if str(av) == 'AT_END':
            return 'end()'
    raise SystemExit(f'unsupported regex construct {name} {av}')


# --- first sets: which chars can start a match (None = can match empty / anything)
def first(data, flags):
    """set of first chars of a match of `data` (a list of nodes), plus whether it can be empty"""
    acc = set()
    for op, av in data:
        name = str(op)
        if name in SINGLE:
            acc |= SET_CPS[node_set((op, av), flags)]
            return acc, False
        if name in ('ASSERT', 'ASSERT_NOT', 'AT'):
            continue
        if name == 'SUBPATTERN':
            f, empty = first(av[3], flags)
        elif name == 'BRANCH':
            f, empty = set(), False
            for b in av[1]:
                fb, eb = first(b, flags)
                f |= fb
                empty = empty or eb
        elif name in ('MAX_REPEAT', 'MIN_REPEAT'):
            f, empty = first(av[2], flags)
            empty = empty or av[0] == 0
        else:
            raise SystemExit(name)
        acc |= f
        if not empty:
            return acc, False
    return acc, True


# --- token types
TYPES = []


def tid(tt):
    if tt not in TYPES:
        TYPES.append(tt)
    return TYPES.index(tt)


for tt in (tokens.Error, tokens.Name, tokens.Literal, tokens.Comment.Multiline, tokens.Comment.Multiline.Hint):
    tid(tt)

lexer = Lexer()
lexer.default_initialization()
rules = []
for rx, tt in keywords.SQL_REGEX:
    parsed = _parser.parse(rx, FLAGS)
    expr = emit(parsed.data, FLAGS)
    f, empty = first(parsed.data, FLAGS)
    assert not empty, rx
    fs = set_id_for(('first', rx), f)
    if tt is keywords.PROCESS_AS_KEYWORD:
        t = 'KW'
    else:
        t = str(tid(tt))
    rules.append((rx, t, fs, expr))

kw = {}
for d in lexer._keywords:
    for k, v in d.items():
        assert k.isascii() and k == k.upper(), k
        if k not in kw:
            kw[k] = tid(v)

# str.upper() for non-ASCII chars whose upper case is pure ASCII
upper = []
for c in CODEPOINTS:
    if c < 128:
        continue
    u = chr(c).upper()
    if u.isascii():
        upper.append((c, u))
isspace = set_id_for(('isspace',), [c for c in CODEPOINTS if chr(c).isspace()])
dq_tag_first = regex_set(r'[_A-ZÀ-Ü]', re.IGNORECASE | re.UNICODE)
dq_word = regex_set(r'\w', re.IGNORECASE | re.UNICODE)
dq_not_opener = regex_set(r'[\w"$]', re.UNICODE)


def flags_of(tt):
    T = tokens
    return {
        'punct': tt is T.Punctuation,
        'in_keyword': tt in T.Keyword,
        'keyword': tt is T.Keyword,
        'ddl': tt is T.Keyword.DDL,
        'name': tt is T.Name,
        'eos': tt in (T.Whitespace, T.Comment.Single),
        'no_reset': tt in (T.Whitespace, T.Newline, T.Comment.Single, T.Comment.Multiline),
        'is_ws': tt in T.Whitespace,
    }


out = []
w = out.append
w('// @generated by gen/gen_tables.py from sqlparse %s on Python %s. Do not edit.' % (sqlparse.__version__, sys.version.split()[0]))
w('#![allow(clippy::all)]')
w('use crate::rx::*;')
w('pub const SQLPARSE_VERSION: &str = "%s";' % sqlparse.__version__)
w('pub const PYTHON_VERSION: &str = "%s";' % sys.version.split()[0])
import unicodedata
w('pub const UNIDATA_VERSION: &str = "%s";' % unicodedata.unidata_version)
w('pub static SETS: &[&[(u32, u32)]] = &[')
for rs in SETS:
    w('    &[' + ', '.join(f'({a}, {b})' for a, b in rs) + '],')
w('];')
w('pub const ISSPACE: usize = %d;' % isspace)
w('pub const DQ_TAG_FIRST: usize = %d;' % dq_tag_first)
w('pub const DQ_WORD: usize = %d;' % dq_word)
w('pub const DQ_NOT_OPENER: usize = %d;' % dq_not_opener)
w('pub const T_ERROR: u8 = 0;\npub const T_NAME: u8 = 1;\npub const T_LITERAL: u8 = 2;\npub const T_COMMENT_ML: u8 = 3;\npub const T_COMMENT_ML_HINT: u8 = 4;')
w('pub static TYPE_NAMES: &[&str] = &[' + ', '.join('"%s"' % repr(t) for t in TYPES) + '];')
w('pub struct TypeFlags { pub punct: bool, pub in_keyword: bool, pub keyword: bool, pub ddl: bool, pub name: bool, pub eos: bool, pub no_reset: bool, pub is_ws: bool }')
w('pub static TYPE_FLAGS: &[TypeFlags] = &[')
for t in TYPES:
    f = flags_of(t)
    w('    TypeFlags { ' + ', '.join(f'{k}: {str(v).lower()}' for k, v in f.items()) + ' },')
w('];')
w('pub static KEYWORDS: &[(&str, u8)] = &[')
for k, v in kw.items():
    w('    ("%s", %d),' % (k.replace('\\', '\\\\').replace('"', '\\"'), v))
w('];')
w('pub static UPPER: &[(u32, &str)] = &[')
for c, u in upper:
    w('    (%d, "%s"),' % (c, u))
w('];')
w('pub const KW: u8 = 255;')
w('/// (pattern, token type or KW, first-char set, regex)')
w('pub fn rules() -> Vec<(&\'static str, u8, usize, Node)> {')
w('    vec![')
for rx, t, fs, expr in rules:
    w('        (%s, %s, %d, %s),' % ('r#"' + rx + '"#', t, fs, expr))
w('    ]')
w('}')
print('\n'.join(out))
print(f'// {len(SETS)} sets, {len(rules)} rules, {len(kw)} keywords, {len(TYPES)} token types', file=sys.stderr)
