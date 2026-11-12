# loaded via -p: route sqlparse to the Rust core for the whole upstream test run
import sqlparse_rs
sqlparse_rs.install()
from sqlparse_rs import _core
calls = {"n": 0}
_t = _core.tokenize
def _counted(*a):
    calls["n"] += 1
    return _t(*a)
_core.tokenize = _counted
_s = _core.split
def _counted_s(*a):
    calls["n"] += 1
    return _s(*a)
_core.split = _counted_s
import atexit
atexit.register(lambda: print(f"\nrust core calls: {calls['n']}"))
