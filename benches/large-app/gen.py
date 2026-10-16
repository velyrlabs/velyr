#!/usr/bin/env python3
"""Generates the large-app fixture (VELYR-183): a server with a ~2,000 module ESM/CJS graph, a
configurable live state and three routes with real work (/json, /transform, /template).

    python3 -I benches/large-app/gen.py            # writes benches/fixtures/large-app
    python3 -I benches/large-app/gen.py --check    # generates twice, compares the tree hashes

Deterministic: a fixed seed and an integer PRNG of our own, so the output does not depend on the
Python version. Standard library only, no network. The runtime files (app.js, server.js, ...) are
copied from benches/large-app/templates, so the hash covers them too.
Plain ES2015 syntax (no generators, private fields, optional chaining): the native engine runs it.
"""
import argparse
import hashlib
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
TEMPLATES = HERE / "templates"
DEFAULT_OUT = ROOT / "benches/fixtures/large-app"
SEED = 20261016
MASK = (1 << 64) - 1
CJS_EVERY = 5  # every 5th module is CommonJS (.cjs)

WORDS = ("parse normalize merge resolve encode decode window bucket ledger invoice account route "
         "session token cursor sample buffer record schema filter score rank cache render").split()


class Rng:
    """xorshift64*: integer only, identical on every Python."""

    def __init__(self, seed):
        self.x = (seed * 0x9E3779B97F4A7C15 + 1) & MASK

    def next(self):
        x = self.x
        x ^= x >> 12
        x ^= (x << 25) & MASK
        x ^= x >> 27
        self.x = x
        return ((x * 0x2545F4914F6CDD1D) & MASK) >> 16

    def below(self, n):
        return self.next() % n

    def between(self, low, high):
        return low + self.below(high - low + 1)

    def pick(self, seq):
        return seq[self.below(len(seq))]


def doc(rng):
    return "/** " + " ".join(rng.pick(WORDS) for _ in range(rng.between(4, 9))) + ". */"


# --- function templates: each returns (source, exported helper name or None) -------------------

def f_hash(r, k):
    return f"""{doc(r)}
function hash{k}(input) {{
  let h = {r.between(1, 1 << 30)} >>> 0;
  for (let i = 0; i < input.length; i++) {{
    h = Math.imul(h ^ input.charCodeAt(i), {r.between(3, 1 << 20) | 1}) >>> 0;
    h ^= h >>> {r.between(5, 17)};
  }}
  return h >>> 0;
}}
""", f"hash{k}"


def f_normalize(r, k):
    return f"""{doc(r)}
function normalize{k}(text, limit) {{
  const max = limit === undefined ? {r.between(40, 400)} : limit;
  const parts = String(text).split(/\\s+/).filter(Boolean);
  const out = [];
  for (const part of parts) {{
    if (out.length >= max) break;
    out.push(part.toLowerCase().replace(/[^a-z0-9]/g, ""));
  }}
  return out.join(" ");
}}
""", f"normalize{k}"


def f_counter(r, k):
    return f"""{doc(r)}
function makeCounter{k}(step) {{
  const inc = step === undefined ? {r.between(1, 9)} : step;
  let n = 0;
  const history = [];
  return {{
    next() {{
      n += inc;
      history.push(n);
      if (history.length > {r.between(8, 64)}) history.shift();
      return n;
    }},
    reset() {{
      n = 0;
      history.length = 0;
    }},
    peek: () => n,
  }};
}}
""", None


def f_table(r, k):
    return f"""const TABLE{k} = Array.from({{ length: {r.between(24, 220)} }}, (_, i) => ((i * {r.between(3, 997)}) ^ {r.between(1, 4095)}) % {r.between(11, 1009)});
{doc(r)}
function lookup{k}(i) {{
  return TABLE{k}[Math.abs(i) % TABLE{k}.length];
}}
""", f"lookup{k}"


def f_pad(r, k):
    return f"""{doc(r)}
function pad{k}(value, width) {{
  let s = String(value);
  while (s.length < width) s = "{r.pick('0 _ . -'.split())}" + s;
  return s;
}}
{doc(r)}
function chunk{k}(list, size) {{
  const out = [];
  for (let i = 0; i < list.length; i += size) out.push(list.slice(i, i + size));
  return out;
}}
""", f"pad{k}"


def f_summarize(r, k):
    return f"""{doc(r)}
function summarize{k}(values) {{
  return values.reduce((acc, v) => {{
    acc.sum += v;
    acc.min = Math.min(acc.min, v);
    acc.max = Math.max(acc.max, v);
    acc.count++;
    return acc;
  }}, {{ sum: 0, min: Infinity, max: -Infinity, count: 0 }});
}}
""", None


def f_class(r, k):
    return f"""{doc(r)}
class Service{k} {{
  constructor(name) {{
    this.name = name;
    this.calls = 0;
    this._cache = new Map();
  }}
  compute(value) {{
    this.calls++;
    const key = this.name + ":" + value;
    if (this._cache.has(key)) return this._cache.get(key);
    let acc = {r.between(1, 99)};
    for (let i = 0; i < {r.between(4, 24)}; i++) acc = (acc * {r.between(3, 31)} + value + i) % {r.between(101, 65521)};
    if (this._cache.size > {r.between(16, 128)}) this._cache.clear();
    this._cache.set(key, acc);
    return acc;
  }}
  describe() {{
    return this.name + "#" + this.calls;
  }}
}}
""", None


FUNCTIONS = [f_hash, f_normalize, f_counter, f_table, f_pad, f_summarize, f_class]


def module_source(r, index, cjs, deps):
    """deps: list of (relative import path, is_cjs) of earlier modules."""
    helpers, hash_name, lookup_name, pad_name = [], None, None, None
    for k, make in enumerate([f_hash, f_table, f_pad] + [r.pick(FUNCTIONS) for _ in range(r.between(3, 7))]):
        source, name = make(r, k)
        helpers.append(source)
        if name and name.startswith("hash") and hash_name is None:
            hash_name = name
        if name and name.startswith("lookup") and lookup_name is None:
            lookup_name = name
        if name and name.startswith("pad") and pad_name is None:
            pad_name = name
    rot = r.between(1, 30)
    mod = r.between(3, 91)
    shift = r.between(1, 25)
    head, links = [], []
    for n, (path, dep_cjs) in enumerate(deps):
        if cjs:
            head.append(f'const dep{n} = require("{path}");')
        elif dep_cjs:
            head.append(f'import dep{n} from "{path}";')
        else:
            head.append(f'import {{ meta as dep{n}meta }} from "{path}";')
        links.append(f"dep{n}.meta.id" if (cjs or dep_cjs) else f"dep{n}meta.id")
    body = f"""
const meta = {{ id: {index}, name: "m{index}", kind: "{'cjs' if cjs else 'esm'}" }};

{doc(r)}
function transform(text) {{
  const s = String(text);
  if (s.length === 0) return s;
  switch ({hash_name}(s) % 4) {{
    case 0:
      return s.slice({rot}) + s.slice(0, {rot});
    case 1:
      return normalize(s);
    case 2:
      return s.split("").reverse().join("");
    default:
      return s.replace(/[aeiou]/g, (c) => String.fromCharCode(((c.charCodeAt(0) - 94 + {mod}) % 26) + 97));
  }}
}}

{doc(r)}
function format(item) {{
  return item.name + " (" + {pad_name}(item.index, 6) + ") #" + {lookup_name}(item.index + {shift});
}}

{doc(r)}
function score(item) {{
  return ((item.attrs.stock * {r.between(2, 17)} + {hash_name}(item.id)) % 1000) / 10;
}}

function register(reg) {{
  reg.modules++;
  reg.transforms.push({{ name: meta.name, fn: transform }});
  reg.formatters.push(format);
  reg.scorers.push(score);
  {chr(10).join(f'  reg.edges.push([meta.id, {link}]);' for link in links).strip()}
}}
"""
    # `normalize` resolves to the first normalize helper of the module, or a local fallback.
    norm = next((s for s in helpers if "function normalize" in s), None)
    if norm is None:
        helpers.append("function normalize(text) {\n  return String(text).toLowerCase();\n}\n")
    else:
        body = body.replace("return normalize(s);", "return " + norm.split("function ")[1].split("(")[0] + "(s);")
    exports = ("module.exports = { meta, register };\n" if cjs else "export { meta, register };\n")
    return "\n".join(head) + ("\n" if head else "") + "\n".join(helpers) + body + exports


def build(groups, per_group, cjs_every=CJS_EVERY):
    """Returns {relative path: text}."""
    rng = Rng(SEED)
    files = {"package.json": '{\n  "name": "large-app",\n  "private": true,\n  "type": "module"\n}\n'}
    for template in sorted(TEMPLATES.iterdir()):
        files[template.name] = template.read_text()
    earlier = []  # (group, name, cjs)
    index = 0
    group_exports = []
    for g in range(groups):
        entries = []
        for j in range(per_group):
            cjs = cjs_every > 0 and index % cjs_every == cjs_every - 1
            ext = "cjs" if cjs else "js"
            name = f"m{j:03d}"
            deps = []
            for _ in range(rng.between(1, 3)):
                if not earlier:
                    break
                pg, pn, pc = rng.pick([e for e in earlier if e[2] or not cjs] or earlier)
                if cjs and not pc:
                    continue
                path = f"./{pn}.{'cjs' if pc else 'js'}" if pg == g else f"../g{pg:02d}/{pn}.{'cjs' if pc else 'js'}"
                if (path, pc) not in deps:
                    deps.append((path, pc))
            files[f"src/g{g:02d}/{name}.{ext}"] = module_source(rng, index, cjs, deps)
            earlier.append((g, name, cjs))
            entries.append((name, cjs))
            index += 1
        lines = []
        for n, (name, cjs) in enumerate(entries):
            lines.append(f'import m{n} from "./{name}.cjs";' if cjs else f'import * as m{n} from "./{name}.js";')
        lines.append("")
        lines.append("export function registerAll(reg) {")
        lines += [f"  m{n}.register(reg);" for n in range(len(entries))]
        lines.append("}")
        files[f"src/g{g:02d}/index.js"] = "\n".join(lines) + "\n"
        group_exports.append(f"g{g:02d}")
    top = [f'import {{ registerAll as r{n} }} from "./{g}/index.js";' for n, g in enumerate(group_exports)]
    top += ["", "export function registerAll(reg) {"] + [f"  r{n}(reg);" for n in range(len(group_exports))] + ["}"]
    files["src/index.js"] = "\n".join(top) + "\n"
    return files


def write(files, out):
    for rel, text in files.items():
        path = out / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, newline="\n")


def tree_hash(root):
    digest = hashlib.sha256()
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        digest.update(path.relative_to(root).as_posix().encode() + b"\0")
        digest.update(path.read_bytes() + b"\0")
    return digest.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--groups", type=int, default=40)
    ap.add_argument("--per-group", type=int, default=50)
    ap.add_argument("--cjs-every", type=int, default=CJS_EVERY, help="every Nth module is CommonJS; 0 = ESM only")
    ap.add_argument("--check", action="store_true", help="generate twice into temp dirs and compare hashes")
    args = ap.parse_args()
    if args.check:
        hashes = []
        for _ in range(2):
            with tempfile.TemporaryDirectory() as tmp:
                write(build(args.groups, args.per_group, args.cjs_every), Path(tmp))
                hashes.append(tree_hash(Path(tmp)))
        print(f"run 1: {hashes[0]}\nrun 2: {hashes[1]}")
        if hashes[0] != hashes[1]:
            sys.exit("error: the generator is not deterministic")
        if args.out.is_dir():
            print("on disk:", "matches" if tree_hash(args.out) == hashes[0] else "differs (run without --check)")
        print("ok")
        return
    files = build(args.groups, args.per_group, args.cjs_every)
    write(files, args.out)
    size = sum(len(t.encode()) for t in files.values())
    print(f"{len(files)} files, {size / 1e6:.1f} MB, {args.groups * args.per_group} modules -> {args.out}")


if __name__ == "__main__":
    main()
