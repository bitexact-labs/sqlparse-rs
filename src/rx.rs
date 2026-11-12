//! A tiny backtracking regex engine with Python `re` (sre) semantics for the constructs
//! sqlparse's lexer uses: char sets (precomputed from CPython), sequences, alternation,
//! greedy/lazy repeats, lookahead, 1-char lookbehind, \b and $.
//! Leftmost-first: alternatives and repeat counts are tried in the order sre tries them.

pub const INF: u32 = u32::MAX;

pub enum Node {
    Set(usize),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Rep { min: u32, max: u32, greedy: bool, body: Box<Node> },
    Ahead { neg: bool, body: Box<Node> },
    Behind { neg: bool, set: usize },
    Boundary,
    End,
    /// greedy unbounded repeat whose body is a choice of fixed sequences of char sets
    RepFixed { min: u32, alts: Vec<Vec<usize>> },
}

fn fixed_seq(n: &Node) -> Option<Vec<usize>> {
    match n {
        Node::Set(i) => Some(vec![*i]),
        Node::Seq(v) => {
            let mut out = vec![];
            for x in v {
                out.extend(fixed_seq(x)?);
            }
            Some(out)
        }
        _ => None,
    }
}

/// rewrite repeats that the specialised matcher can run without closures
pub fn optimize(n: Node) -> Node {
    match n {
        Node::Seq(v) => Node::Seq(v.into_iter().map(optimize).collect()),
        Node::Alt(v) => Node::Alt(v.into_iter().map(optimize).collect()),
        Node::Ahead { neg, body } => Node::Ahead { neg, body: Box::new(optimize(*body)) },
        Node::Rep { min, max, greedy, body } => {
            if max == INF && greedy && !matches!(*body, Node::Set(_)) {
                let alts = match &*body {
                    Node::Alt(v) => v.iter().map(fixed_seq).collect::<Option<Vec<_>>>(),
                    other => fixed_seq(other).map(|s| vec![s]),
                };
                if let Some(alts) = alts {
                    if alts.len() <= 6 && alts.iter().all(|a| !a.is_empty()) {
                        return Node::RepFixed { min, alts };
                    }
                }
            }
            Node::Rep { min, max, greedy, body: Box::new(optimize(*body)) }
        }
        other => other,
    }
}

pub fn set(i: usize) -> Node {
    Node::Set(i)
}
pub fn seq(v: Vec<Node>) -> Node {
    if v.len() == 1 {
        return v.into_iter().next().unwrap();
    }
    Node::Seq(v)
}
pub fn alt(v: Vec<Node>) -> Node {
    Node::Alt(v)
}
pub fn rep(min: u32, max: u32, greedy: bool, body: Node) -> Node {
    Node::Rep { min, max, greedy, body: Box::new(body) }
}
pub fn ahead(neg: bool, body: Node) -> Node {
    Node::Ahead { neg, body: Box::new(body) }
}
pub fn behind(neg: bool, set: usize) -> Node {
    Node::Behind { neg, set }
}
pub fn boundary() -> Node {
    Node::Boundary
}
pub fn end() -> Node {
    Node::End
}

pub struct CharSet {
    ascii: [u64; 2],
    ranges: &'static [(u32, u32)],
}

impl CharSet {
    pub fn new(ranges: &'static [(u32, u32)]) -> Self {
        let mut ascii = [0u64; 2];
        for &(a, b) in ranges {
            for c in a..=b.min(127) {
                ascii[(c >> 6) as usize] |= 1 << (c & 63);
            }
        }
        CharSet { ascii, ranges }
    }
    #[inline]
    pub fn has(&self, c: char) -> bool {
        let c = c as u32;
        if c < 128 {
            return self.ascii[(c >> 6) as usize] & (1 << (c & 63)) != 0;
        }
        let i = self.ranges.partition_point(|r| r.1 < c);
        i < self.ranges.len() && self.ranges[i].0 <= c
    }
}

pub struct Matcher<'a> {
    pub text: &'a [char],
    pub sets: &'a [CharSet],
    pub word: usize,
    /// set when a repeat needed more backtracking state than supported
    pub overflow: std::cell::Cell<bool>,
}

struct Frame {
    p: usize,
    ends: [usize; 6],
    n: u8,
    i: u8,
}

impl<'a> Matcher<'a> {
    #[inline]
    fn is_word(&self, pos: isize) -> bool {
        pos >= 0 && (pos as usize) < self.text.len() && self.sets[self.word].has(self.text[pos as usize])
    }

    /// match `node` at `pos`, then the continuation `k` with the end position
    pub fn m(&self, node: &Node, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        let t = self.text;
        match node {
            Node::Set(i) => pos < t.len() && self.sets[*i].has(t[pos]) && k(pos + 1),
            Node::Seq(v) => self.seq(v, 0, pos, k),
            Node::Alt(v) => {
                for b in v {
                    if self.m(b, pos, k) {
                        return true;
                    }
                }
                false
            }
            Node::Rep { min, max, greedy, body } => {
                if let Node::Set(si) = **body {
                    // single-char body: count the run, then try counts in sre's order
                    let s = &self.sets[si];
                    let min = *min as usize;
                    let max = *max as u64;
                    if *greedy {
                        let mut run = 0usize;
                        while (run as u64) < max && pos + run < t.len() && s.has(t[pos + run]) {
                            run += 1;
                        }
                        if run < min {
                            return false;
                        }
                        let mut c = run;
                        loop {
                            if k(pos + c) {
                                return true;
                            }
                            if c == min {
                                return false;
                            }
                            c -= 1;
                        }
                    } else {
                        // lazy: extend one char at a time, like sre (no look at the rest of the run)
                        let mut c = 0usize;
                        while c < min {
                            if pos + c < t.len() && s.has(t[pos + c]) {
                                c += 1;
                            } else {
                                return false;
                            }
                        }
                        loop {
                            if k(pos + c) {
                                return true;
                            }
                            if (c as u64) < max && pos + c < t.len() && s.has(t[pos + c]) {
                                c += 1;
                            } else {
                                return false;
                            }
                        }
                    }
                } else if *max != INF {
                    self.rep_rec(body, 0, *min, *max, *greedy, pos, k)
                } else {
                    self.rep_dfs(body, *min, *greedy, pos, k)
                }
            }
            Node::Ahead { neg, body } => {
                let r = self.m(body, pos, &mut |_| true);
                r != *neg && k(pos)
            }
            Node::Behind { neg, set } => {
                let r = pos > 0 && self.sets[*set].has(t[pos - 1]);
                r != *neg && k(pos)
            }
            Node::Boundary => {
                let p = pos as isize;
                !t.is_empty() && self.is_word(p - 1) != self.is_word(p) && k(pos)
            }
            Node::End => (pos == t.len() || (pos + 1 == t.len() && t[pos] == '\n')) && k(pos),
            Node::RepFixed { min, alts } => self.rep_fixed(alts, *min, pos, k),
        }
    }

    fn seq(&self, v: &[Node], i: usize, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        if i == v.len() {
            return k(pos);
        }
        self.m(&v[i], pos, &mut |p| self.seq(v, i + 1, p, k))
    }

    #[allow(clippy::too_many_arguments)]
    fn rep_rec(&self, body: &Node, count: u32, min: u32, max: u32, greedy: bool, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        if greedy {
            if count < max && self.m(body, pos, &mut |p| p != pos && self.rep_rec(body, count + 1, min, max, greedy, p, k)) {
                return true;
            }
            count >= min && k(pos)
        } else {
            if count >= min && k(pos) {
                return true;
            }
            count < max && self.m(body, pos, &mut |p| p != pos && self.rep_rec(body, count + 1, min, max, greedy, p, k))
        }
    }

    /// unbounded greedy repeat of a complex body, iteratively (no recursion per iteration).
    /// The continuation depends only on the position, so states known to fail are memoised.
    fn rep_dfs(&self, body: &Node, min: u32, greedy: bool, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        if !greedy {
            self.overflow.set(true);
            return false;
        }
        let ends_of = |p: usize| -> Option<Frame> {
            let mut f = Frame { p, ends: [0; 6], n: 0, i: 0 };
            let mut over = false;
            self.m(body, p, &mut |e| {
                if e != p {
                    if (f.n as usize) < f.ends.len() {
                        f.ends[f.n as usize] = e;
                        f.n += 1;
                    } else {
                        over = true;
                    }
                }
                false
            });
            if over {
                self.overflow.set(true);
                return None;
            }
            Some(f)
        };
        let mut failed: std::collections::HashSet<(usize, u32)> = std::collections::HashSet::new();
        let mut stack: Vec<Frame> = Vec::new();
        match ends_of(pos) {
            Some(f) => stack.push(f),
            None => return false,
        }
        loop {
            let depth = stack.len();
            if depth == 0 {
                return false;
            }
            let top = &mut stack[depth - 1];
            if top.i < top.n {
                let e = top.ends[top.i as usize];
                top.i += 1;
                // e is reached after `depth` iterations
                if failed.contains(&(e, (depth as u32).min(min))) {
                    continue;
                }
                match ends_of(e) {
                    Some(f) => stack.push(f),
                    None => return false,
                }
                continue;
            }
            // every further iteration from here failed: leave the repeat here
            let p = top.p;
            let cnt = (depth - 1) as u32;
            if cnt >= min && k(p) {
                return true;
            }
            failed.insert((p, cnt.min(min)));
            stack.pop();
        }
    }

    #[inline]
    fn fixed_ends(&self, alts: &[Vec<usize>], p: usize) -> Frame {
        let t = self.text;
        let mut f = Frame { p, ends: [0; 6], n: 0, i: 0 };
        for a in alts {
            if p + a.len() <= t.len() && a.iter().enumerate().all(|(j, &si)| self.sets[si].has(t[p + j])) {
                f.ends[f.n as usize] = p + a.len();
                f.n += 1;
            }
        }
        f
    }

    /// same search order and memo as rep_dfs, ends computed directly
    fn rep_fixed(&self, alts: &[Vec<usize>], min: u32, pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        // fast path: the first branch of the search always takes the first alternative that
        // fits; its first exit attempt is where none fits. Only if that fails, search fully.
        let t = self.text;
        let mut p = pos;
        let mut cnt = 0u32;
        'walk: loop {
            for a in alts {
                if p + a.len() <= t.len() && a.iter().enumerate().all(|(j, &si)| self.sets[si].has(t[p + j])) {
                    p += a.len();
                    cnt += 1;
                    continue 'walk;
                }
            }
            break;
        }
        if cnt >= min && k(p) {
            return true;
        }
        let mut failed: std::collections::HashSet<(usize, u32)> = std::collections::HashSet::new();
        let mut stack: Vec<Frame> = vec![self.fixed_ends(alts, pos)];
        loop {
            let depth = stack.len();
            if depth == 0 {
                return false;
            }
            let top = &mut stack[depth - 1];
            if top.i < top.n {
                let e = top.ends[top.i as usize];
                top.i += 1;
                if !failed.is_empty() && failed.contains(&(e, (depth as u32).min(min))) {
                    continue;
                }
                let f = self.fixed_ends(alts, e);
                stack.push(f);
                continue;
            }
            let p = top.p;
            let cnt = (depth - 1) as u32;
            if cnt >= min && k(p) {
                return true;
            }
            failed.insert((p, cnt.min(min)));
            stack.pop();
        }
    }

    pub fn full(&self, node: &Node, pos: usize) -> Option<usize> {
        let mut end = None;
        self.m(node, pos, &mut |p| {
            end = Some(p);
            true
        });
        end
    }
}
