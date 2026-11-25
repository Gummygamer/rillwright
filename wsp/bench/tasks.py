"""Programming and terminal tasks for measuring weight edits (see wsp/README.md).

Each task: id, kind ("prog" or "term"), prompt, setup(ws) that builds the fixture, and
check(ws, answer) -> (ok, detail). Fixtures are deterministic. Tests used by a checker are hidden
from the agent unless the task puts them in the fixture.

Split: tasks alternate practice / heldout within each kind, in list order. Only practice tasks may
be used while looking for an edit; heldout tasks are run once per candidate at the end.
"""
import hashlib
import io
import json
import os
import random
import re
import stat
import subprocess
import sys
import tarfile


def w(ws, rel, text, mode=None):
    p = os.path.join(ws, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w") as f:
        f.write(text)
    if mode is not None:
        os.chmod(p, mode)


def rd(ws, rel):
    try:
        with open(os.path.join(ws, rel)) as f:
            return f.read()
    except (OSError, UnicodeDecodeError):
        return None


def py(ws, code, timeout=30):
    """Runs hidden test code in the workspace. The code must print OK on success."""
    try:
        r = subprocess.run([sys.executable, "-c", code], cwd=ws, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return False, "hidden test timed out"
    if r.returncode == 0 and r.stdout.strip().endswith("OK"):
        return True, "hidden tests pass"
    tail = (r.stderr.strip().splitlines() or r.stdout.strip().splitlines() or ["no output"])[-1]
    return False, tail[:200]


def sh(ws, argv, timeout=30):
    try:
        return subprocess.run(argv, cwd=ws, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, 124, "", "timed out")


def hidden(code):
    return lambda ws, ans: py(ws, code)


def answer_has(pattern, what):
    return lambda ws, ans: (re.search(pattern, ans) is not None, f"answer should contain {what}")


def rng(name):
    return random.Random(int(hashlib.sha256(name.encode()).hexdigest()[:8], 16))


WORDS = ("amber basil cedar delta ember fjord grove harbor iris jade kelp lotus maple nectar olive "
         "pearl quartz river sage tulip umber violet willow xenon yarrow zephyr").split()

# ---- programming ---------------------------------------------------------------------------------

def s_lru(ws):
    w(ws, "lru.py", "# LRU cache goes here\n")

T_LRU = """
from lru import LRUCache
c = LRUCache(2)
c.put('a', 1); c.put('b', 2)
assert c.get('a') == 1
c.put('c', 3)
assert c.get('b') is None
assert c.get('a') == 1 and c.get('c') == 3
c.put('a', 10); c.put('d', 4)
assert c.get('c') is None and c.get('a') == 10 and c.get('d') == 4
c1 = LRUCache(1); c1.put(1, 1); c1.put(2, 2)
assert c1.get(1) is None and c1.get(2) == 2
print('OK')
"""

def s_intervals(ws):
    w(ws, "intervals.py", "")

T_INTERVALS = """
from intervals import merge_intervals as m
assert m([]) == []
assert m([(1, 3), (2, 6), (8, 10), (15, 18)]) == [(1, 6), (8, 10), (15, 18)]
assert m([(5, 7), (1, 2), (2, 3)]) == [(1, 3), (5, 7)]
assert m([(1, 10), (2, 3), (4, 5)]) == [(1, 10)]
assert m([(1, 2), (4, 5)]) == [(1, 2), (4, 5)]
print('OK')
"""

def s_roman(ws):
    w(ws, "roman.py", "")

T_ROMAN = """
from roman import to_roman, from_roman
cases = {1: 'I', 4: 'IV', 9: 'IX', 14: 'XIV', 40: 'XL', 90: 'XC', 400: 'CD', 1994: 'MCMXCIV', 2024: 'MMXXIV', 3999: 'MMMCMXCIX'}
for n, r in cases.items():
    assert to_roman(n) == r, (n, to_roman(n))
    assert from_roman(r) == n, (r, from_roman(r))
for n in range(1, 400):
    assert from_roman(to_roman(n)) == n
print('OK')
"""

def s_bsearch(ws):
    w(ws, "search.py", """def find_first(xs, target):
    \"\"\"Index of the first occurrence of target in the sorted list xs, or -1.\"\"\"
    lo, hi = 0, len(xs) - 1
    while lo < hi:
        mid = (lo + hi) // 2
        if xs[mid] < target:
            lo = mid
        else:
            hi = mid
    return lo if xs[lo] == target else -1
""")
    w(ws, "test_search.py", """import unittest
from search import find_first


class T(unittest.TestCase):
    def test_found(self):
        self.assertEqual(find_first([1, 2, 2, 2, 5], 2), 1)
        self.assertEqual(find_first([1, 3, 5, 7], 7), 3)

    def test_missing(self):
        self.assertEqual(find_first([1, 3, 5], 4), -1)


if __name__ == '__main__':
    unittest.main()
""")

T_BSEARCH = """
from search import find_first as f
assert f([1, 2, 2, 2, 5], 2) == 1
assert f([1, 3, 5, 7], 7) == 3
assert f([1, 3, 5], 4) == -1
assert f([], 3) == -1
assert f([4], 4) == 0 and f([4], 5) == -1
assert f([1, 1, 1, 1], 1) == 0
assert f(list(range(0, 100, 2)), 98) == 49
print('OK')
"""

def s_csv(ws):
    w(ws, "csvline.py", "")

T_CSV = '''
from csvline import parse_csv_line as p
assert p('a,b,c') == ['a', 'b', 'c']
assert p('a,"b,c",d') == ['a', 'b,c', 'd']
assert p('"say ""hi""",x') == ['say "hi"', 'x']
assert p('a,,c') == ['a', '', 'c']
assert p('') == ['']
assert p('"",z') == ['', 'z']
assert p('one') == ['one']
print('OK')
'''

def s_pricing(ws):
    w(ws, "shop/__init__.py", "")
    w(ws, "shop/pricing.py", """def discount(total, percent):
    \"\"\"Price after taking percent off.\"\"\"
    return total - percent


def with_tax(total, rate=0.2):
    return round(total * (1 + rate), 2)
""")
    w(ws, "shop/cart.py", """from shop.pricing import discount, with_tax


class Cart:
    def __init__(self):
        self.items = []

    def add(self, name, price, qty=1):
        self.items.append((name, price, qty))

    def subtotal(self):
        return sum(price for _, price, qty in self.items)

    def total(self, percent_off=0):
        return with_tax(discount(self.subtotal(), percent_off))
""")
    w(ws, "test_cart.py", """import unittest
from shop.cart import Cart


class T(unittest.TestCase):
    def test_total(self):
        c = Cart()
        c.add('pen', 2.0, qty=5)
        c.add('book', 10.0)
        self.assertEqual(c.subtotal(), 20.0)
        self.assertEqual(c.total(percent_off=50), 12.0)


if __name__ == '__main__':
    unittest.main()
""")

T_PRICING = """
from shop.cart import Cart
from shop.pricing import discount
c = Cart(); c.add('pen', 2.0, qty=5); c.add('book', 10.0)
assert c.subtotal() == 20.0
assert c.total(percent_off=50) == 12.0
assert c.total() == 24.0
assert discount(200, 10) == 180
c2 = Cart(); c2.add('x', 3.0, qty=3)
assert c2.subtotal() == 9.0
print('OK')
"""

def s_wordfreq(ws):
    w(ws, "sample.txt", "the cat and the dog and the bird\nA cat saw a Dog\n")

def c_wordfreq(ws, ans):
    r = rng("wordfreq")
    words = ["kiwi"] * 9 + ["plum"] * 7 + ["Fig"] * 4 + ["fig"] * 3 + ["pear"] * 5 + ["lime"] * 2
    r.shuffle(words)
    w(ws, "_hidden_in.txt", "\n".join(" ".join(words[i:i + 5]) for i in range(0, len(words), 5)) + "\n")
    p = sh(ws, [sys.executable, "wordfreq.py", "_hidden_in.txt"])
    os.remove(os.path.join(ws, "_hidden_in.txt"))
    got = p.stdout.strip().splitlines()
    return got == ["kiwi 9", "fig 7", "plum 7"], f"got {got[:4]!r} {p.stderr.strip()[-80:]}"

def s_flatten(ws):
    w(ws, "flat.py", "")

T_FLATTEN = """
from flat import flatten
assert flatten({}) == {}
assert flatten({'a': 1}) == {'a': 1}
assert flatten({'a': {'b': 1, 'c': {'d': 2}}, 'e': 3}) == {'a.b': 1, 'a.c.d': 2, 'e': 3}
assert flatten({'a': {'b': [1, 2]}}) == {'a.b': [1, 2]}
assert flatten({'x': {}}) == {}
assert flatten({'a': {'b': 1}}, sep='/') == {'a/b': 1}
print('OK')
"""

def s_twobugs(ws):
    w(ws, "textstats/__init__.py", "")
    w(ws, "textstats/tokens.py", """def tokenize(text):
    \"\"\"Lowercase words, punctuation removed.\"\"\"
    out = []
    for raw in text.split():
        word = raw.strip('.,!?;:')
        if word:
            out.append(word)
    return out
""")
    w(ws, "textstats/stats.py", """from textstats.tokens import tokenize


def average_word_length(text):
    words = tokenize(text)
    if not words:
        return 0.0
    return sum(len(w) for w in words) / len(text)


def longest_word(text):
    words = tokenize(text)
    return max(words, key=len) if words else ''
""")
    w(ws, "tests/__init__.py", "")
    w(ws, "tests/test_stats.py", """import unittest
from textstats.stats import average_word_length, longest_word
from textstats.tokens import tokenize


class T(unittest.TestCase):
    def test_tokenize(self):
        self.assertEqual(tokenize('Hello, World!'), ['hello', 'world'])

    def test_average(self):
        self.assertEqual(average_word_length('aa bbbb'), 3.0)

    def test_longest(self):
        self.assertEqual(longest_word('a quick fox.'), 'quick')


if __name__ == '__main__':
    unittest.main()
""")

T_TWOBUGS = """
from textstats.stats import average_word_length, longest_word
from textstats.tokens import tokenize
assert tokenize('Hello, World!') == ['hello', 'world']
assert tokenize('  A  b. ') == ['a', 'b']
assert average_word_length('aa bbbb') == 3.0
assert average_word_length('One, two.') == 3.0
assert average_word_length('') == 0.0
assert longest_word('a quick fox.') == 'quick'
print('OK')
"""

def s_calc(ws):
    w(ws, "calc.py", "")

T_CALC = """
from calc import evaluate as e
assert e('1 + 2') == 3
assert e('2 + 3 * 4') == 14
assert e('(2 + 3) * 4') == 20
assert e('10 - 4 - 3') == 3
assert e('2 * (3 + (4 - 1)) * 2') == 24
assert e('8 / 2 / 2') == 2
assert e('7') == 7
assert e(' 12*(3+4)-5 ') == 79
assert e('-3 + 5') == 2
print('OK')
"""

def s_sep(ws):
    w(ws, "names.py", """def join_names(names):
    return ', '.join(names)
""")
    w(ws, "report.py", """from names import join_names


def header(team):
    return 'Team: ' + join_names(team)


def path_line(parts):
    # TODO: should be joined with ' > ' once join_names supports a separator
    return join_names(parts)
""")

T_SEP = """
from names import join_names
from report import header, path_line
assert join_names(['a', 'b']) == 'a, b'
assert join_names(['a', 'b'], sep=' | ') == 'a | b'
assert join_names([], sep='-') == ''
assert header(['ann', 'bo']) == 'Team: ann, bo'
assert path_line(['home', 'docs', 'x']) == 'home > docs > x'
print('OK')
"""

def s_semver(ws):
    w(ws, "semver.py", "")

T_SEMVER = """
from semver import compare as c
assert c('1.0.0', '1.0.0') == 0
assert c('1.2.3', '1.2.10') == -1
assert c('2.0.0', '1.99.99') == 1
assert c('1.10.0', '1.9.9') == 1
assert c('1.0.0-alpha', '1.0.0') == -1
assert c('1.0.0', '1.0.0-rc.1') == 1
assert c('1.0.0-alpha', '1.0.0-beta') == -1
assert c('0.0.1', '0.0.2') == -1
print('OK')
"""

def s_keyerror(ws):
    orders = [{"id": 1, "amount": 12.5, "status": "paid"}, {"id": 2, "amount": 7.25, "status": "paid"},
              {"id": 3, "status": "cancelled"}, {"id": 4, "amount": 30.0, "status": "paid"},
              {"id": 5, "amount": 4.75, "status": "refunded"}]
    w(ws, "orders.json", json.dumps(orders, indent=1) + "\n")
    w(ws, "report.py", """import json
import sys


def total_paid(orders):
    total = 0
    for o in orders:
        if o['status'] == 'paid' or o['amount'] > 0:
            total += o['amount']
    return total


if __name__ == '__main__':
    with open(sys.argv[1]) as f:
        print('total paid: %.2f' % total_paid(json.load(f)))
""")

def c_keyerror(ws, ans):
    p = sh(ws, [sys.executable, "report.py", "orders.json"])
    return p.returncode == 0 and "49.75" in p.stdout, f"stdout {p.stdout.strip()[:60]!r} {p.stderr.strip()[-80:]}"

def s_dedupe(ws):
    w(ws, "dedupe.py", "")

T_DEDUPE = """
from dedupe import dedupe
assert dedupe([]) == []
assert dedupe(['a', 'B', 'A', 'b', 'c']) == ['a', 'B', 'c']
assert dedupe(['x', 'x', 'X']) == ['x']
assert dedupe(['Hello', 'world', 'HELLO', 'World', '!']) == ['Hello', 'world', '!']
print('OK')
"""

def s_matrix(ws):
    w(ws, "matrix.py", "")

T_MATRIX = """
from matrix import transpose, multiply
assert transpose([[1, 2, 3], [4, 5, 6]]) == [[1, 4], [2, 5], [3, 6]]
assert transpose([]) == []
assert multiply([[1, 2], [3, 4]], [[5, 6], [7, 8]]) == [[19, 22], [43, 50]]
assert multiply([[1, 2, 3]], [[1], [2], [3]]) == [[14]]
try:
    multiply([[1, 2]], [[1, 2]])
    raise SystemExit('expected ValueError')
except ValueError:
    pass
print('OK')
"""

def s_backup(ws):
    w(ws, "src/a.txt", "alpha\n")
    w(ws, "src/b.txt", "beta\n")
    w(ws, "src/c.md", "not copied\n")
    w(ws, "src/sub/d.txt", "nested, not copied\n")

def c_backup(ws, ans):
    import shutil
    shutil.rmtree(os.path.join(ws, "backup"), ignore_errors=True)
    w(ws, "src/new file.txt", "gamma\n")
    p = sh(ws, ["bash", "backup.sh"])
    got = sorted(os.listdir(os.path.join(ws, "backup"))) if os.path.isdir(os.path.join(ws, "backup")) else None
    ok = p.returncode == 0 and got == ["a.txt", "b.txt", "new file.txt"] and rd(ws, "backup/new file.txt") == "gamma\n"
    return ok, f"backup/ has {got} {p.stderr.strip()[-80:]}"

def s_rle(ws):
    w(ws, "rle.py", "")

T_RLE = """
from rle import encode, decode
assert encode('') == ''
assert encode('aaabccdddd') == '3a1b2c4d'
assert encode('abc') == '1a1b1c'
assert decode('3a1b2c4d') == 'aaabccdddd'
assert decode('12x') == 'x' * 12
assert decode(encode('z' * 25 + 'yy')) == 'z' * 25 + 'yy'
print('OK')
"""

def s_greet(ws):
    w(ws, "greet.py", """import sys

if __name__ == '__main__':
    print('Hello, world!')
""")

def c_greet(ws, ans):
    a = sh(ws, [sys.executable, "greet.py"]).stdout.strip()
    b = sh(ws, [sys.executable, "greet.py", "--name", "Ada"]).stdout.strip()
    c = sh(ws, [sys.executable, "greet.py", "--shout", "--name", "Ada"]).stdout.strip()
    d = sh(ws, [sys.executable, "greet.py", "--shout"]).stdout.strip()
    got = [a, b, c, d]
    return got == ["Hello, world!", "Hello, Ada!", "HELLO, ADA!", "HELLO, WORLD!"], f"got {got}"

# ---- terminal ------------------------------------------------------------------------------------

def s_loglines(ws):
    r = rng("loglines")
    for i, n in enumerate([137, 52, 208, 91]):
        w(ws, f"logs/app{i}.log", "".join(f"line {r.randint(0, 9999)}\n" for _ in range(n)))
    w(ws, "logs/readme.txt", "not a log\n" * 40)
    w(ws, "logs/old/app.log.bak", "x\n" * 13)

def s_largest(ws):
    r = rng("largest")
    sizes = {"data/a/report.bin": 4100, "data/a/b/cache.dat": 9731, "data/notes.txt": 800,
             "data/c/image.raw": 9450, "data/c/d/e/dump.sql": 9120, "data/zz.tmp": 3000}
    for rel, n in sizes.items():
        w(ws, rel, "".join(r.choice("abcdef0123456789") for _ in range(n)))

def s_tar(ws):
    w(ws, "src/main.py", "print('main')\n")
    w(ws, "src/util/helpers.py", "def helper():\n    return 1\n")
    w(ws, "README.md", "# project\n")

def c_tar(ws, ans):
    p = os.path.join(ws, "out.tar.gz")
    if not os.path.isfile(p):
        return False, "out.tar.gz missing"
    try:
        with tarfile.open(p, "r:gz") as t:
            names = {n.lstrip("./") for n in t.getnames()}
    except tarfile.TarError as e:
        return False, f"not a gzip tar: {e}"
    ok = {"src/main.py", "src/util/helpers.py"} <= names and "README.md" not in names
    return ok, f"members {sorted(names)[:6]}"

def s_git(ws):
    w(ws, "app.py", "print('app')\n")
    w(ws, "lib/core.py", "X = 1\n")

def c_git(ws, ans):
    env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null")
    def g(*a):
        return subprocess.run(["git", *a], cwd=ws, capture_output=True, text=True, env=env)
    log = g("log", "--format=%s")
    files = set(g("ls-tree", "-r", "--name-only", "HEAD").stdout.split())
    ok = log.returncode == 0 and log.stdout.strip().splitlines() == ["initial"] and {"app.py", "lib/core.py"} <= files
    return ok, f"log {log.stdout.strip()[:40]!r} files {sorted(files)[:4]} {log.stderr.strip()[-60:]}"

TOKEN_RUN = "run-" + hashlib.sha256(b"wsp-run").hexdigest()[:10]

def s_runscript(ws):
    h = hashlib.sha256(b"wsp-run").hexdigest()[:10]
    w(ws, "tools/token.sh", f"#!/bin/sh\n# prints the build token\na={h[:5]}\nb={h[5:]}\necho \"run-$a$b\"\n", mode=0o644)

def s_grepfind(ws):
    r = rng("grepfind")
    target = (3, 17)
    for d in range(6):
        for i in range(5):
            lines = [f"INFO {r.randint(1000, 9999)} ok" for _ in range(40)]
            lines[r.randint(0, 39)] = f"ERROR {r.randint(1000, 5520)} failed"
            if (d, i) == (target[0], 2):
                lines[target[1]] = "ERROR 5521 disk quota exceeded"
            w(ws, f"var/svc{d}/{'nested/' if d % 2 else ''}unit{i}.log", "\n".join(lines) + "\n")

def s_rename(ws):
    for rel in ["photos/a.jpeg", "photos/trip/b.jpeg", "photos/trip/day2/c.jpeg", "photos/keep.png", "photos/trip/d.jpg",
                "photos/jpeg_notes.txt"]:
        w(ws, rel, rel + "\n")

def c_rename(ws, ans):
    have = sorted(os.path.relpath(os.path.join(d, f), ws) for d, _, fs in os.walk(os.path.join(ws, "photos")) for f in fs)
    want = sorted(["photos/a.jpg", "photos/trip/b.jpg", "photos/trip/day2/c.jpg", "photos/keep.png", "photos/trip/d.jpg",
                   "photos/jpeg_notes.txt"])
    return have == want and rd(ws, "photos/trip/day2/c.jpg") == "photos/trip/day2/c.jpeg\n", f"have {have}"

def _csv_rows():
    r = rng("sumcol")
    return [(WORDS[i % len(WORDS)] + str(i), r.randint(100, 99999), r.randint(1, 50)) for i in range(240)]

def s_sumcol(ws):
    w(ws, "data.csv", "name,amount,qty\n" + "".join(f"{a},{b},{c}\n" for a, b, c in _csv_rows()))

SUMCOL = sum(b for _, b, _ in _csv_rows())

def s_clean(ws):
    for rel in ["pkg/__pycache__/a.cpython-312.pyc", "pkg/sub/__pycache__/b.cpython-312.pyc", "pkg/stray.pyc",
                "pkg/a.py", "pkg/sub/b.py", "pkg/sub/data.pyc.txt", "__pycache__/top.cpython-312.pyc", "main.py"]:
        w(ws, rel, "x\n")

def c_clean(ws, ans):
    have = sorted(os.path.relpath(os.path.join(d, f), ws) for d, _, fs in os.walk(ws) for f in fs if ".dory" not in d)
    dirs = [d for d, _, _ in os.walk(ws) if os.path.basename(d) == "__pycache__"]
    want = sorted(["pkg/a.py", "pkg/sub/b.py", "pkg/sub/data.pyc.txt", "main.py"])
    return have == want and not dirs, f"files {have} pycache dirs {len(dirs)}"

def _blob():
    r = rng("sha")
    return "".join(r.choice("abcdefghijklmnopqrstuvwxyz \n") for _ in range(5000))

def s_sha(ws):
    w(ws, "release/payload.bin", _blob())

SHA8 = hashlib.sha256(_blob().encode()).hexdigest()[:8]

def _access():
    r = rng("ips")
    ips = [f"10.{r.randint(0, 3)}.{r.randint(0, 255)}.{r.randint(1, 254)}" for _ in range(83)]
    return [f'{r.choice(ips)} - - [10/Nov/2026:10:{r.randint(0, 59):02d}:00] "GET /p{r.randint(1, 30)} HTTP/1.1" 200 {r.randint(100, 9000)}'
            for _ in range(420)]

def s_ips(ws):
    w(ws, "access.log", "\n".join(_access()) + "\n")

NIPS = len({l.split()[0] for l in _access()})

def s_sed(ws):
    w(ws, "etc/app.conf", "url = http://localhost:8080/api\ntimeout = 5\n")
    w(ws, "etc/proxy/up.conf", "upstream localhost:8080;\nbackup localhost:8081;\n")
    w(ws, "etc/proxy/readme.txt", "docs mention localhost:8080 and must not change\n")
    w(ws, "etc/db.conf", "host = localhost\nport = 5432\n")

def c_sed(ws, ans):
    ok = (rd(ws, "etc/app.conf") == "url = http://api.internal:9090/api\ntimeout = 5\n"
          and rd(ws, "etc/proxy/up.conf") == "upstream api.internal:9090;\nbackup localhost:8081;\n"
          and rd(ws, "etc/proxy/readme.txt") == "docs mention localhost:8080 and must not change\n"
          and rd(ws, "etc/db.conf") == "host = localhost\nport = 5432\n")
    return ok, "the two .conf files changed, nothing else"

def s_mkdirs(ws):
    w(ws, "project.toml", '[project]\nname = "widget"\nversion = "3.14.2"\n\n[tool]\nversion = "0.0.1"\n')

def c_mkdirs(ws, ans):
    dirs = all(os.path.isdir(os.path.join(ws, d)) for d in ["build/debug/obj", "build/release/obj"])
    v = (rd(ws, "build/VERSION") or "").strip()
    return dirs and v == "3.14.2", f"dirs {dirs} VERSION {v!r}"

def s_bigfiles(ws):
    r = rng("bigfiles")
    sizes = [300, 1024, 1025, 5000, 980, 2048, 77, 1500, 1023, 4096, 12, 1100, 640]
    for i, n in enumerate(sizes):
        w(ws, f"tree/{'x/' * (i % 3)}f{i}.dat", "".join(r.choice("01") for _ in range(n)))

NBIG = sum(1 for n in [300, 1024, 1025, 5000, 980, 2048, 77, 1500, 1023, 4096, 12, 1100, 640] if n > 1024)

def _names():
    r = rng("names")
    base = ["Mara", "otto", "Lena", "ivan", "Zoe", "abel", "Quinn", "bela", "Hugo", "nora", "Cleo", "dara"]
    out = base + [n.upper() for n in base[:5]] + [n.lower() for n in base[3:9]] + base[:4]
    r.shuffle(out)
    return out

def s_sortuniq(ws):
    w(ws, "names.txt", "\n".join(_names()) + "\n")

def c_sortuniq(ws, ans):
    got = (rd(ws, "sorted.txt") or "").split("\n")
    got = [g for g in got if g]
    want = sorted({n.lower() for n in _names()})
    return [g.lower() for g in got] == want, f"{len(got)} lines, want {len(want)}"

TOKEN_TAR = "vault-" + hashlib.sha256(b"wsp-tar").hexdigest()[:10]

def s_untar(ws):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        for name, text in [("bundle/readme.txt", "see inner\n"), ("bundle/inner/secret.txt", TOKEN_TAR + "\n"),
                           ("bundle/inner/other.txt", "nothing\n")]:
            data = text.encode()
            info = tarfile.TarInfo(name)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))
    os.makedirs(ws, exist_ok=True)
    with open(os.path.join(ws, "archive.tar.gz"), "wb") as f:
        f.write(buf.getvalue())

def _biglines():
    r = rng("line137")
    return [f"{i + 1:04d}-{''.join(r.choice('abcdefghjkmnpqrstuvwxyz') for _ in range(8))}" for i in range(400)]

def s_line(ws):
    # the visible numbering is shifted, so the line must be found by position
    w(ws, "big.txt", "\n".join(l[5:] for l in _biglines()) + "\n")

LINE137 = _biglines()[136][5:]

def s_chmod(ws):
    for rel in ["scripts/build.sh", "scripts/deploy.sh", "scripts/lib/common.sh"]:
        w(ws, rel, "#!/bin/sh\necho hi\n", mode=0o600)
    for rel in ["scripts/README.md", "scripts/lib/data.json"]:
        w(ws, rel, "x\n", mode=0o777)

def c_chmod(ws, ans):
    modes = {}
    for d, _, fs in os.walk(os.path.join(ws, "scripts")):
        for f in fs:
            p = os.path.join(d, f)
            modes[os.path.relpath(p, ws)] = stat.S_IMODE(os.stat(p).st_mode)
    ok = all(m == (0o755 if rel.endswith(".sh") else 0o644) for rel, m in modes.items()) and len(modes) == 5
    return ok, " ".join(f"{os.path.basename(k)}={oct(v)[2:]}" for k, v in sorted(modes.items()))

def s_failcount(ws):
    body = "import unittest\n\n\nclass T(unittest.TestCase):\n"
    for i, (a, b) in enumerate([(1, 1), (2, 2), (3, 4), (5, 5), (8, 8), (13, 12), (21, 21), (34, 35), (55, 55)]):
        body += f"    def test_case_{i}(self):\n        self.assertEqual({a}, {b})\n\n"
    w(ws, "test_numbers.py", body + "\nif __name__ == '__main__':\n    unittest.main()\n")
    w(ws, "test_strings.py", "import unittest\n\n\nclass S(unittest.TestCase):\n    def test_upper(self):\n        self.assertEqual('a'.upper(), 'A')\n\n    def test_strip(self):\n        self.assertEqual(' a '.strip(), 'a ')\n")

def _book():
    r = rng("book")
    counts = {"lantern": 41, "harbor": 38, "the": 0, "willow": 33, "ember": 29}
    words = [wd for wd, n in counts.items() for _ in range(n)] + [r.choice(WORDS[5:]) for _ in range(300)]
    words = [x for x in words if x not in ("harbor",)] + ["harbor"] * 20
    r.shuffle(words)
    return words

def s_topword(ws):
    ws_words = _book()
    w(ws, "book.txt", "\n".join(" ".join(ws_words[i:i + 9]) for i in range(0, len(ws_words), 9)) + "\n")

def _top():
    c = {}
    for x in _book():
        c[x] = c.get(x, 0) + 1
    return max(c.items(), key=lambda kv: kv[1])

def s_json2csv(ws):
    r = rng("json2csv")
    rows = [{"name": WORDS[i].title(), "age": r.randint(18, 80), "city": r.choice(["Oslo", "Lima", "Pune"])} for i in range(12)]
    rows[4]["name"] = "Mc, Rae"
    w(ws, "people.json", json.dumps(rows, indent=1) + "\n")

def c_json2csv(ws, ans):
    import csv
    src = json.loads(rd(ws, "people.json"))
    text = rd(ws, "people.csv")
    if text is None:
        return False, "people.csv missing"
    rows = list(csv.reader(io.StringIO(text)))
    want = [["name", "age"]] + [[p["name"], str(p["age"])] for p in src]
    return [r for r in rows if r] == want, f"{len(rows)} rows, first {rows[:2]}"

def s_dups(ws):
    r = rng("dups")
    for i in range(14):
        w(ws, f"dup/{'m/' if i % 4 == 0 else ''}file{i:02d}.txt", "".join(r.choice("abcdefgh\n") for _ in range(600)))
    w(ws, "dup/m/copy_of_report.txt", rd(ws, "dup/file09.txt"))

TASKS = [
    # --- programming
    dict(id="p_lru", kind="prog", setup=s_lru, check=hidden(T_LRU),
         prompt="In lru.py, write a class LRUCache(capacity) with get(key), which returns the value or None, and put(key, value). When the cache is over capacity it drops the least recently used key. Both get and put count as a use."),
    dict(id="p_intervals", kind="prog", setup=s_intervals, check=hidden(T_INTERVALS),
         prompt="In intervals.py, write merge_intervals(intervals). It takes a list of (start, end) tuples in any order and returns a sorted list of tuples with overlapping or touching intervals merged."),
    dict(id="p_roman", kind="prog", setup=s_roman, check=hidden(T_ROMAN),
         prompt="In roman.py, write to_roman(n) and from_roman(s) for the numbers 1 to 3999."),
    dict(id="p_bsearch", kind="prog", setup=s_bsearch, check=hidden(T_BSEARCH),
         prompt="find_first in search.py is wrong and can hang. Fix it so that python3 -m unittest passes, and make sure it also works for an empty list."),
    dict(id="p_csv", kind="prog", setup=s_csv, check=hidden(T_CSV),
         prompt="In csvline.py, write parse_csv_line(line) without using the csv module. It splits one CSV line into a list of fields. Fields may be wrapped in double quotes, a quoted field may contain commas, and two double quotes inside a quoted field mean one double quote."),
    dict(id="p_pricing", kind="prog", setup=s_pricing, check=hidden(T_PRICING),
         prompt="python3 -m unittest fails in this project. Find the bugs and fix them so the tests pass. Do not change the tests."),
    dict(id="p_wordfreq", kind="prog", setup=s_wordfreq, check=c_wordfreq,
         prompt="Write wordfreq.py. Run as `python3 wordfreq.py FILE`, it prints the three most common words in FILE, one per line as `word count`, most common first, with ties in alphabetical order. Words are compared in lowercase. Try it on sample.txt."),
    dict(id="p_flatten", kind="prog", setup=s_flatten, check=hidden(T_FLATTEN),
         prompt="In flat.py, write flatten(d, sep='.'). It turns a nested dict into a flat dict whose keys are the paths joined by sep, for example {'a': {'b': 1}} becomes {'a.b': 1}. Lists are kept as values. An empty nested dict adds nothing."),
    dict(id="p_twobugs", kind="prog", setup=s_twobugs, check=hidden(T_TWOBUGS),
         prompt="Run python3 -m unittest in this project and fix the code in textstats/ until every test passes. Do not change the tests."),
    dict(id="p_calc", kind="prog", setup=s_calc, check=hidden(T_CALC),
         prompt="In calc.py, write evaluate(expr) without using eval. It computes an arithmetic expression given as a string with integers, + - * /, parentheses, spaces and a leading minus sign, with the usual precedence. Division results that are whole numbers must compare equal to the integer."),
    dict(id="p_sep", kind="prog", setup=s_sep, check=hidden(T_SEP),
         prompt="Give join_names in names.py an optional sep argument that defaults to ', '. Then do the TODO in report.py."),
    dict(id="p_semver", kind="prog", setup=s_semver, check=hidden(T_SEMVER),
         prompt="In semver.py, write compare(a, b) for versions like '1.2.3' or '1.2.3-rc.1'. It returns -1, 0 or 1. Numbers compare as numbers. A version with a pre-release part is lower than the same version without one, and two pre-release parts compare as strings."),
    dict(id="p_keyerror", kind="prog", setup=s_keyerror, check=c_keyerror,
         prompt="`python3 report.py orders.json` crashes. Fix report.py so it prints the total of the orders whose status is paid."),
    dict(id="p_dedupe", kind="prog", setup=s_dedupe, check=hidden(T_DEDUPE),
         prompt="In dedupe.py, write dedupe(items). It takes a list of strings and returns a list without duplicates, comparing case-insensitively, keeping the first spelling seen and the original order."),
    dict(id="p_matrix", kind="prog", setup=s_matrix, check=hidden(T_MATRIX),
         prompt="In matrix.py, write transpose(m) and multiply(a, b) for matrices given as lists of rows. multiply raises ValueError when the shapes do not fit."),
    dict(id="p_backup", kind="prog", setup=s_backup, check=c_backup,
         prompt="Write backup.sh. Run as `bash backup.sh`, it copies every .txt file directly inside src/ (not in subfolders) into backup/, creating backup/ if needed. File names may contain spaces. Run it once to check it works."),
    dict(id="p_rle", kind="prog", setup=s_rle, check=hidden(T_RLE),
         prompt="In rle.py, write encode(s) and decode(s) for run-length encoding: encode('aaabcc') is '3a1b2c'. Counts can have more than one digit. The input to encode has no digits."),
    dict(id="p_greet", kind="prog", setup=s_greet, check=c_greet,
         prompt="Change greet.py to use argparse. --name NAME greets that name instead of world, and --shout prints the greeting in upper case. Without options the output stays `Hello, world!`."),
    # --- terminal
    dict(id="t_loglines", kind="term", setup=s_loglines, check=answer_has(r"\b488\b", "488"),
         prompt="How many lines are there in total in the .log files directly inside logs/? Answer with the number."),
    dict(id="t_largest", kind="term", setup=s_largest, check=answer_has(r"cache\.dat", "cache.dat"),
         prompt="Which file under data/ is the largest? Answer with its path."),
    dict(id="t_tar", kind="term", setup=s_tar, check=c_tar,
         prompt="Create out.tar.gz containing the src folder and nothing else."),
    dict(id="t_git", kind="term", setup=s_git, check=c_git,
         prompt="Make this folder a git repository and commit all its files in one commit with the message: initial"),
    dict(id="t_runscript", kind="term", setup=s_runscript, check=answer_has(re.escape(TOKEN_RUN), "the token the script prints"),
         prompt="Run tools/token.sh and tell me exactly what it prints."),
    dict(id="t_grepfind", kind="term", setup=s_grepfind, check=answer_has(r"svc3/nested/unit2\.log", "var/svc3/nested/unit2.log"),
         prompt="One log file under var/ contains the text `ERROR 5521`. Which file is it? Answer with its path."),
    dict(id="t_rename", kind="term", setup=s_rename, check=c_rename,
         prompt="Rename every .jpeg file under photos/, including subfolders, so it ends in .jpg instead. Leave other files alone."),
    dict(id="t_sumcol", kind="term", setup=s_sumcol, check=answer_has(rf"\b{SUMCOL}\b|\b{SUMCOL:,}\b", str(SUMCOL)),
         prompt="What is the sum of the amount column in data.csv? Answer with the number."),
    dict(id="t_clean", kind="term", setup=s_clean, check=c_clean,
         prompt="Delete every __pycache__ folder and every .pyc file in this project. Do not delete anything else."),
    dict(id="t_sha", kind="term", setup=s_sha, check=answer_has(SHA8, "the first 8 hex digits"),
         prompt="What are the first 8 hex digits of the SHA-256 of release/payload.bin?"),
    dict(id="t_ips", kind="term", setup=s_ips, check=answer_has(rf"\b{NIPS}\b", str(NIPS)),
         prompt="How many different client IP addresses appear in access.log? The address is the first field of each line. Answer with the number."),
    dict(id="t_sed", kind="term", setup=s_sed, check=c_sed,
         prompt="In every .conf file under etc/, replace localhost:8080 with api.internal:9090. Do not touch files that are not .conf files."),
    dict(id="t_mkdirs", kind="term", setup=s_mkdirs, check=c_mkdirs,
         prompt="Create the folders build/debug/obj and build/release/obj, and write the project's version from project.toml into build/VERSION."),
    dict(id="t_bigfiles", kind="term", setup=s_bigfiles, check=answer_has(rf"\b{NBIG}\b", str(NBIG)),
         prompt="How many files under tree/ are larger than 1024 bytes? Answer with the number."),
    dict(id="t_sortuniq", kind="term", setup=s_sortuniq, check=c_sortuniq,
         prompt="names.txt has one name per line, with repeats in different letter case. Write sorted.txt with each name once, in lowercase, sorted alphabetically."),
    dict(id="t_untar", kind="term", setup=s_untar, check=answer_has(re.escape(TOKEN_TAR), "the text in secret.txt"),
         prompt="archive.tar.gz contains a file named secret.txt somewhere inside. What does that file say?"),
    dict(id="t_line", kind="term", setup=s_line, check=answer_has(LINE137, "line 137"),
         prompt="What is on line 137 of big.txt? Answer with the exact text of that line."),
    dict(id="t_chmod", kind="term", setup=s_chmod, check=c_chmod,
         prompt="Under scripts/, including subfolders, set every .sh file to mode 755 and every other file to mode 644."),
    dict(id="t_failcount", kind="term", setup=s_failcount, check=answer_has(r"\b4\b", "4"),
         prompt="Run python3 -m unittest here. How many tests fail? Answer with the number. Do not fix anything."),
    dict(id="t_topword", kind="term", setup=s_topword, check=lambda ws, ans: (re.search(rf"\b{_top()[0]}\b", ans.lower()) is not None, f"answer should name {_top()[0]}"),
         prompt="Which word appears most often in book.txt? Answer with the word."),
    dict(id="t_json2csv", kind="term", setup=s_json2csv, check=c_json2csv,
         prompt="Convert people.json to people.csv with a header line and the columns name and age, in that order, keeping the row order. Quote fields where CSV needs it."),
    dict(id="t_dups", kind="term", setup=s_dups, check=lambda ws, ans: ("file09.txt" in ans and "copy_of_report.txt" in ans, "answer should name file09.txt and copy_of_report.txt"),
         prompt="Two files under dup/ have identical contents. Which two? Answer with both paths."),
]

_seen = {"prog": 0, "term": 0}
for _t in TASKS:
    _t["split"] = "practice" if _seen[_t["kind"]] % 2 == 0 else "heldout"
    _seen[_t["kind"]] += 1
