//! Rust core of sqlparse's lexer (sqlparse.lexer.Lexer.get_tokens with the default
//! configuration) and statement splitter (sqlparse.split), BSD-3-Clause like sqlparse.
use pyo3::prelude::*;
use pyo3::types::{PyList, PyString, PyTuple};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::OnceLock;

/// FxHash with a final mix (keyword lookups on short strings)
#[derive(Default)]
struct Fx(u64);
impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x517cc1b727220a95);
        }
    }
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
}

mod gen;
mod rx;
use gen::*;
use rx::{CharSet, Matcher, Node};

pyo3::create_exception!(_core, Unsupported, pyo3::exceptions::PyException);

struct Tables {
    sets: Vec<CharSet>,
    rules: Vec<(u8, usize, Node)>,
    /// candidate rules for each ASCII first char
    ascii_rules: Vec<Vec<u16>>,
    keywords: HashMap<&'static str, u8, BuildHasherDefault<Fx>>,
    upper: HashMap<u32, &'static str>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let sets: Vec<CharSet> = SETS.iter().map(|r| CharSet::new(r)).collect();
        let rules: Vec<(u8, usize, Node)> = gen::rules().into_iter().map(|(_, t, f, n)| (t, f, rx::optimize(n))).collect();
        let mut ascii_rules = vec![vec![]; 128];
        for (c, list) in ascii_rules.iter_mut().enumerate() {
            for (i, r) in rules.iter().enumerate() {
                if sets[r.1].has(char::from(c as u8)) {
                    list.push(i as u16);
                }
            }
        }
        Tables {
            sets,
            rules,
            ascii_rules,
            keywords: KEYWORDS.iter().copied().collect(),
            upper: UPPER.iter().copied().collect(),
        }
    })
}

/// str.upper() as far as comparisons with ASCII words need it: chars whose upper case is
/// not ASCII become U+FFFF (never equal to an ASCII letter). Returns (upper, is_ascii).
fn upper(tb: &Tables, s: &[char]) -> (String, bool) {
    let mut out = String::with_capacity(s.len());
    let mut ascii = true;
    for &c in s {
        if c.is_ascii() {
            out.push(c.to_ascii_uppercase());
        } else if let Some(u) = tb.upper.get(&(c as u32)) {
            out.push_str(u);
        } else {
            out.push('\u{FFFF}');
            ascii = false;
        }
    }
    (out, ascii)
}

/// Lexer.is_keyword: KEYWORDS lookup of value.upper(), else Name
fn keyword_type(tb: &Tables, w: &[char]) -> u8 {
    let mut buf = [0u8; 48];
    let mut n = 0usize;
    for &c in w {
        if c.is_ascii() {
            if n == buf.len() {
                return T_NAME;
            }
            buf[n] = (c as u8).to_ascii_uppercase();
            n += 1;
        } else if let Some(u) = tb.upper.get(&(c as u32)) {
            for b in u.bytes() {
                if n == buf.len() {
                    return T_NAME;
                }
                buf[n] = b;
                n += 1;
            }
        } else {
            return T_NAME; // upper case is not ASCII: no keyword can match
        }
    }
    let key = std::str::from_utf8(&buf[..n]).unwrap();
    tb.keywords.get(key).copied().unwrap_or(T_NAME)
}

struct Tok {
    t: u8,
    a: usize,
    b: usize,
}

/// keywords.find_delimited_spans + DelimitedSpans.resolve
struct Spans {
    openers: HashMap<usize, (usize, usize, u8)>, // start -> (opener end, tag id, ttype)
    closers: Vec<(Vec<usize>, Vec<usize>)>,       // tag id -> (starts, ends)
}

fn find_spans(tb: &Tables, t: &[char]) -> Spans {
    let mut sp = Spans { openers: HashMap::new(), closers: vec![] };
    let has_dollar = t.contains(&'$');
    let has_comment = t.windows(2).any(|w| w[0] == '/' && w[1] == '*');
    if !has_dollar && !has_comment {
        return sp;
    }
    let mut tags: HashMap<String, usize> = HashMap::new();
    let (first, word, notop) = (&tb.sets[DQ_TAG_FIRST], &tb.sets[DQ_WORD], &tb.sets[DQ_NOT_OPENER]);
    if has_dollar {
        for p in 0..t.len() {
            if t[p] != '$' {
                continue;
            }
            // \$(?:[_A-ZÀ-Ü]\w*)?\$
            let mut q = p + 1;
            let end;
            if q < t.len() && first.has(t[q]) {
                q += 1;
                while q < t.len() && word.has(t[q]) {
                    q += 1;
                }
                if q < t.len() && t[q] == '$' {
                    end = q + 1;
                } else {
                    continue;
                }
            } else if q < t.len() && t[q] == '$' {
                end = q + 1;
            } else {
                continue;
            }
            let tag: String = t[p..end].iter().collect();
            let n = tags.len();
            let id = *tags.entry(tag).or_insert(n);
            if id == sp.closers.len() {
                sp.closers.push((vec![], vec![]));
            }
            sp.closers[id].0.push(p);
            sp.closers[id].1.push(end);
            if p == 0 || !notop.has(t[p - 1]) {
                sp.openers.insert(p, (end, id, T_LITERAL));
            }
        }
    }
    if has_comment {
        let id = sp.closers.len();
        sp.closers.push((vec![], vec![]));
        let mut p = 0;
        while p + 1 < t.len() {
            if t[p] == '/' && t[p + 1] == '*' {
                if p + 2 < t.len() && t[p + 2] == '+' {
                    sp.openers.insert(p, (p + 3, id, T_COMMENT_ML_HINT));
                    p += 3;
                } else {
                    sp.openers.insert(p, (p + 2, id, T_COMMENT_ML));
                    p += 2;
                }
                continue;
            }
            p += 1;
        }
        let mut p = 0;
        while p + 1 < t.len() {
            if t[p] == '*' && t[p + 1] == '/' {
                sp.closers[id].0.push(p);
                sp.closers[id].1.push(p + 2);
                p += 2;
                continue;
            }
            p += 1;
        }
    }
    sp
}

fn lex(t: &[char]) -> PyResult<Vec<Tok>> {
    let tb = tables();
    let spans = find_spans(tb, t);
    let m = Matcher { text: t, sets: &tb.sets, word: DQ_WORD, overflow: std::cell::Cell::new(false) };
    let mut out = Vec::with_capacity(t.len() / 3 + 4);
    let mut pos = 0;
    let n_rules = tb.rules.len();
    while pos < t.len() {
        if let Some(&(oend, tag, tt)) = if spans.openers.is_empty() { None } else { spans.openers.get(&pos) } {
            let (starts, ends) = &spans.closers[tag];
            let idx = starts.partition_point(|&s| s < oend);
            if idx < starts.len() {
                out.push(Tok { t: tt, a: pos, b: ends[idx] });
                pos = ends[idx];
                continue;
            }
        }
        let c = t[pos];
        let mut matched = false;
        let try_rule = |i: usize, pos: usize, out: &mut Vec<Tok>| -> Option<usize> {
            let (tt, _, node) = &tb.rules[i];
            let end = m.full(node, pos)?;
            let tt = if *tt == KW {
                keyword_type(tb, &t[pos..end])
            } else {
                *tt
            };
            out.push(Tok { t: tt, a: pos, b: end });
            Some(end)
        };
        if c.is_ascii() {
            for &i in &tb.ascii_rules[c as usize] {
                if let Some(e) = try_rule(i as usize, pos, &mut out) {
                    pos = e;
                    matched = true;
                    break;
                }
            }
        } else {
            for i in 0..n_rules {
                if !tb.sets[tb.rules[i].1].has(c) {
                    continue;
                }
                if let Some(e) = try_rule(i, pos, &mut out) {
                    pos = e;
                    matched = true;
                    break;
                }
            }
        }
        if m.overflow.get() {
            return Err(Unsupported::new_err("regex backtracking state too large"));
        }
        if !matched {
            out.push(Tok { t: T_ERROR, a: pos, b: pos + 1 });
            pos += 1;
        } else if out.last().map(|k| k.a == k.b).unwrap_or(false) {
            return Err(Unsupported::new_err("empty token"));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ splitter

#[derive(Default)]
struct Splitter {
    block: Vec<&'static str>,
    paren: i64,
    unconfirmed: Option<&'static str>,
    is_create: bool,
    seen_begin: bool,
}

impl Splitter {
    fn nested(&mut self, u: &str) -> Option<i64> {
        match u {
            "FOR" => {
                self.unconfirmed = Some("FOR");
                return Some(0);
            }
            "WHILE" => {
                self.unconfirmed = Some("WHILE");
                return Some(0);
            }
            _ => {}
        }
        if u == "LOOP" || u == "DO" {
            if let Some(s) = self.unconfirmed {
                if s == "FOR" || s == "WHILE" {
                    self.block.push(s);
                    self.unconfirmed = None;
                    return Some(1);
                }
            }
            if u == "LOOP" {
                self.block.push("LOOP");
                return Some(1);
            }
        }
        if u == "IF" || u == "CASE" {
            self.block.push(if u == "IF" { "IF" } else { "CASE" });
            return Some(1);
        }
        None
    }

    fn closing(&mut self, u: &str) -> i64 {
        let last = self.block.last().copied();
        let pop_if = |s: &mut Self, ok: bool| -> i64 {
            if ok {
                s.block.pop();
                -1
            } else {
                0
            }
        };
        match u {
            "END IF" => pop_if(self, last == Some("IF")),
            "END FOR" => pop_if(self, last == Some("FOR")),
            "END WHILE" => pop_if(self, last == Some("WHILE")),
            "END LOOP" => pop_if(self, matches!(last, Some("LOOP") | Some("FOR") | Some("WHILE"))),
            "END CASE" => pop_if(self, last == Some("CASE")),
            "END" => {
                self.block.pop();
                -1
            }
            _ => 0,
        }
    }

    fn change(&mut self, tb: &Tables, tt: u8, v: &[char]) -> i64 {
        let f = &TYPE_FLAGS[tt as usize];
        let single = |c: char| v.len() == 1 && v[0] == c;
        if f.punct && single(';') {
            self.unconfirmed = None;
            if self.seen_begin {
                self.seen_begin = false;
                if self.block.last() == Some(&"BEGIN") {
                    self.block.pop();
                    return -1;
                }
            }
            return 0;
        }
        if f.punct && single('(') {
            self.paren += 1;
            return 1;
        } else if f.punct && single(')') {
            self.paren = (self.paren - 1).max(0);
            return -1;
        } else if !f.in_keyword {
            return 0;
        }
        let (u, _) = upper(tb, v);
        let u = u.as_str();
        if f.ddl && u.starts_with("CREATE") {
            self.is_create = true;
            return 0;
        }
        if u == "DECLARE" && self.is_create && self.block.is_empty() {
            self.block.push("DECLARE");
            return 1;
        }
        if u == "BEGIN" {
            self.seen_begin = true;
            if self.block.last() == Some(&"DECLARE") {
                self.block.pop();
                self.block.push("BEGIN");
                return 0;
            } else {
                self.block.push("BEGIN");
                return 1;
            }
        }
        if self.seen_begin
            && (f.keyword || f.name)
            && matches!(u, "TRANSACTION" | "WORK" | "TRAN" | "DISTRIBUTED" | "DEFERRED" | "IMMEDIATE" | "EXCLUSIVE")
        {
            self.seen_begin = false;
            if self.block.last() == Some(&"BEGIN") {
                self.block.pop();
                return -1;
            }
            return 0;
        }
        if self.block.contains(&"BEGIN") {
            if let Some(r) = self.nested(u) {
                return r;
            }
        }
        self.closing(u)
    }
}

/// StatementSplitter.process: token index ranges of the statements
fn split_tokens(t: &[char], toks: &[Tok]) -> Vec<(usize, usize)> {
    let tb = tables();
    let space = &tb.sets[ISSPACE];
    let mut out = vec![];
    let mut sp = Splitter::default();
    let mut level: i64 = 0;
    let mut consume_ws = false;
    let mut start = 0usize;
    for (i, tk) in toks.iter().enumerate() {
        let f = &TYPE_FLAGS[tk.t as usize];
        let v = &t[tk.a..tk.b];
        if consume_ws && !f.eos {
            out.push((start, i));
            start = i;
            sp = Splitter::default();
            level = 0;
            consume_ws = false;
        }
        level += sp.change(tb, tk.t, v);
        if f.punct && v.len() == 1 && v[0] == ';' {
            sp.seen_begin = false;
            if level <= 0 && !sp.block.contains(&"BEGIN") {
                consume_ws = true;
            }
        } else if f.keyword && {
            // value.split()[0] == 'GO'
            let w: Vec<char> = v.iter().copied().skip_while(|&c| space.has(c)).take_while(|&c| !space.has(c)).collect();
            w == ['G', 'O']
        } {
            consume_ws = true;
        } else if !f.no_reset && !(f.keyword && upper(tb, v).0 == "BEGIN") {
            sp.seen_begin = false;
        }
    }
    if start < toks.len() && !toks[start..].iter().all(|k| TYPE_FLAGS[k.t as usize].is_ws) {
        out.push((start, toks.len()));
    }
    out
}

fn chars_of(s: &Bound<'_, PyString>) -> PyResult<(String, Vec<char>, Vec<usize>)> {
    let st = s.to_str().map_err(|_| Unsupported::new_err("string with surrogates"))?;
    let mut chars = Vec::with_capacity(st.len());
    let mut offs = Vec::with_capacity(st.len() + 1);
    for (o, c) in st.char_indices() {
        chars.push(c);
        offs.push(o);
    }
    offs.push(st.len());
    Ok((st.to_string(), chars, offs))
}

/// tokenize(text, types) -> [(ttype, value), ...] like list(sqlparse.lexer.tokenize(text))
#[pyfunction]
fn tokenize<'py>(py: Python<'py>, text: &Bound<'py, PyString>, types: &Bound<'py, PyList>) -> PyResult<Bound<'py, PyList>> {
    let (s, chars, offs) = chars_of(text)?;
    let toks = lex(&chars)?;
    let tys: Vec<Bound<'py, PyAny>> = types.iter().collect();
    let out = PyList::empty(py);
    for tk in &toks {
        let v = PyString::new(py, &s[offs[tk.a]..offs[tk.b]]);
        out.append(PyTuple::new(py, [tys[tk.t as usize].clone(), v.into_any()])?)?;
    }
    Ok(out)
}

/// split(text, strip_semicolon) -> list of statements like sqlparse.split
#[pyfunction]
#[pyo3(signature = (text, strip_semicolon=false))]
fn split<'py>(py: Python<'py>, text: &Bound<'py, PyString>, strip_semicolon: bool) -> PyResult<Bound<'py, PyList>> {
    let (s, chars, offs) = chars_of(text)?;
    let toks = lex(&chars)?;
    let tb = tables();
    let space = &tb.sets[ISSPACE];
    let out = PyList::empty(py);
    for (i, mut j) in split_tokens(&chars, &toks) {
        if strip_semicolon {
            while j > i {
                let k = &toks[j - 1];
                if TYPE_FLAGS[k.t as usize].is_ws || (k.b - k.a == 1 && chars[k.a] == ';') {
                    j -= 1;
                } else {
                    break;
                }
            }
        }
        if j == i {
            out.append("")?;
            continue;
        }
        let (mut a, mut b) = (toks[i].a, toks[j - 1].b);
        while a < b && space.has(chars[a]) {
            a += 1;
        }
        while b > a && space.has(chars[b - 1]) {
            b -= 1;
        }
        out.append(PyString::new(py, &s[offs[a]..offs[b]]))?;
    }
    Ok(out)
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(tokenize, m)?)?;
    m.add_function(wrap_pyfunction!(split, m)?)?;
    m.add("Unsupported", m.py().get_type::<Unsupported>())?;
    m.add("TYPE_NAMES", TYPE_NAMES.to_vec())?;
    m.add("SQLPARSE_VERSION", SQLPARSE_VERSION)?;
    m.add("UNIDATA_VERSION", UNIDATA_VERSION)?;
    Ok(())
}
