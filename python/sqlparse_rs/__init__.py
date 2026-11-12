"""Drop-in speedup for sqlparse 0.6.1.

``split()`` and ``tokenize()`` run a Rust port of sqlparse's lexer (the same regex rules,
with character classes taken from CPython's ``re``) and of its StatementSplitter, and return
exactly what sqlparse returns. ``install()`` also routes the default lexer of sqlparse to the
Rust core, which speeds up ``sqlparse.parse`` and ``sqlparse.format`` (their grouping and
formatting stay in Python).

Anything outside the fast path (bytes or file input, custom lexer configuration,
strings with lone surrogates) runs the original code.
"""
import sqlparse as _sp
from sqlparse import tokens as _T
from sqlparse import keywords as _kw
from sqlparse.lexer import Lexer as _Lexer
from io import TextIOBase as _TextIOBase

from . import _core

__version__ = '0.1.0'
if _sp.__version__ != _core.SQLPARSE_VERSION:
    raise ImportError(f'sqlparse_rs is built for sqlparse {_core.SQLPARSE_VERSION}, found {_sp.__version__}')


import unicodedata as _ud
# character classes were taken from CPython with this Unicode database; with another one,
# only pure-ASCII input is guaranteed to classify the same, the rest goes to sqlparse itself
_EXACT_UNICODE = _ud.unidata_version == _core.UNIDATA_VERSION


def _fast(text):
    return isinstance(text, str) and (_EXACT_UNICODE or text.isascii())


def _ttype(name):
    t = _T.Token
    for part in name.split('.')[1:]:
        t = getattr(t, part)
    return t


_TYPES = [_ttype(n) for n in _core.TYPE_NAMES]
_Unsupported = _core.Unsupported
_orig_split = _sp.split
_orig_get_tokens = _Lexer.get_tokens

_default_kw = None


def _is_default(lexer):
    """the lexer still has its default configuration (same regex list and keyword dicts)"""
    global _default_kw
    if _default_kw is None:
        _default_kw = [(id(d), len(d)) for d in (
            _kw.KEYWORDS_COMMON, _kw.KEYWORDS_ORACLE, _kw.KEYWORDS_MYSQL, _kw.KEYWORDS_PLPGSQL,
            _kw.KEYWORDS_HQL, _kw.KEYWORDS_MSACCESS, _kw.KEYWORDS_SNOWFLAKE, _kw.KEYWORDS_BIGQUERY,
            _kw.KEYWORDS)]
    rx = lexer._SQL_REGEX
    if len(rx) != len(_kw.SQL_REGEX):
        return False
    for (m, tt), (src, tt2) in zip(rx, _kw.SQL_REGEX):
        if tt is not tt2 or m.__self__.pattern != src:
            return False
    return [(id(d), len(d)) for d in lexer._keywords] == _default_kw


def tokenize(sql, encoding=None):
    """Same as sqlparse.lexer.tokenize, returns a list."""
    lexer = _Lexer.get_default_instance()
    if _fast(sql) and _is_default(lexer):
        try:
            return _core.tokenize(sql, _TYPES)
        except _Unsupported:
            pass
    return list(_orig_get_tokens(lexer, sql, encoding))


def split(sql, encoding=None, strip_semicolon=False):
    """Same as sqlparse.split."""
    if _fast(sql) and _is_default(_Lexer.get_default_instance()):
        try:
            return _core.split(sql, bool(strip_semicolon))
        except _Unsupported:
            pass
    return _orig_split(sql, encoding, strip_semicolon)


def _get_tokens(self, text, encoding=None):
    # a generator, like the original (lexing happens on the first next())
    if _fast(text) and _is_default(self):
        try:
            toks = _core.tokenize(text, _TYPES)
        except _Unsupported:
            toks = None
        if toks is not None:
            yield from toks
            return
    yield from _orig_get_tokens(self, text, encoding)


split.__doc__ = _orig_split.__doc__


def install():
    """Make sqlparse itself use the Rust core (split, parse, format, tokenize)."""
    _sp.split = split
    _Lexer.get_tokens = _get_tokens


def uninstall():
    _sp.split = _orig_split
    _Lexer.get_tokens = _orig_get_tokens
