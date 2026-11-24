#!/usr/bin/env python3
"""Writes the licence notices the app shows in Settings > Licences.

    python3 scripts/licences.py           # write app/Paw/Settings/licences.json
    python3 scripts/licences.py --check   # fail if that file is stale or incomplete

Components are the crates linked into the iOS library: the normal
dependencies of paw-core for aarch64-apple-ios with the default features (a
superset of the Release build, which drops `sim`). Proc macros and build
dependencies are left out, as their code is not in the library. Added by hand:
the Rust standard library, libgit2's C (vendored by libgit2-sys) and the
Public Suffix List data compiled into the psl crate.

Texts are the licence files shipped in each package. A package with none gets
the standard SPDX text from scripts/licence-texts/. Any licence that is not
permissive is listed under "concerns" with what it asks of paw.

Needs cargo and the crates in the cargo registry (`cargo fetch` does that).
Uses only the Python standard library.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "app/Paw/Settings/licences.json"
SPDX_TEXTS = ROOT / "scripts/licence-texts"
TARGET = "aarch64-apple-ios"

LICENCE_FILE = re.compile(r"^(LICEN[SC]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)", re.I)

# Licence files below a package's top folder that cover code compiled in.
EXTRA_FILES = {
    "ring": [
        "third_party/fiat/LICENSE",
        "src/polyfill/once_cell/LICENSE-APACHE",
        "src/polyfill/once_cell/LICENSE-MIT",
    ],
}

PERMISSIVE = {
    "0BSD", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
    "BSD-3-Clause", "BSL-1.0", "bzip2-1.0.6", "CC0-1.0", "CDLA-Permissive-2.0",
    "ISC", "MIT", "MIT-0", "Unicode-3.0", "Unicode-DFS-2016", "Unlicense", "Zlib",
}

# What a licence that is not permissive asks of an app that links it.
REQUIRES = {
    "MPL-2.0": (
        "File-level copyleft. Ship this notice and the MPL text, and tell people "
        "where the source of these files is: {source}. Changed files must be "
        "published under MPL-2.0. The rest of paw can stay MIT, and the App Store "
        "is fine."
    ),
    "libgit2": (
        "libgit2 itself: GPL-2.0 with its linking exception, so linking it "
        "unchanged into paw needs only this notice; changes to libgit2's own "
        "files would fall under the GPL. Its bundled xdiff (deps/xdiff, compiled "
        "in) is LGPL-2.1-or-later: static linking in an iOS app means offering "
        "what lets someone relink paw with a changed xdiff (paw's object files "
        "or its source, which is public), plus the LGPL text. The LGPL's terms on "
        "redistribution sit badly with App Store terms; decide before listing. "
        "PCRE2 (BSD-3-Clause), llhttp and sha1dc (MIT) are permissive. Source: "
        "{source}."
    ),
}


def run_metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked",
         "--filter-platform", TARGET,
         "--manifest-path", str(ROOT / "core/Cargo.toml")],
        check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def linked_packages(meta):
    """The packages reachable from paw-core over normal edges, minus proc macros."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"] or next(
        p["id"] for p in meta["packages"] if p["name"] == "paw-core")

    def proc_macro(p):
        return any("proc-macro" in t["kind"] for t in p["targets"])

    seen, stack = set(), [root]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            normal = any(k["kind"] is None for k in dep["dep_kinds"])
            if normal and not proc_macro(packages[dep["pkg"]]):
                stack.append(dep["pkg"])
    seen.discard(root)
    return [packages[i] for i in seen]


def read_text(path):
    text = path.read_text(encoding="utf-8", errors="replace")
    return text.replace("\r\n", "\n").strip("\n") + "\n"


def spdx_ids(expression):
    """The licence ids in an expression, `WITH` exceptions kept attached."""
    expression = expression.replace("/", " OR ")
    parts = re.split(r"\s+(?:OR|AND)\s+|[()]", expression)
    return [p.strip() for p in parts if p.strip()]


def is_permissive(expression):
    """True if some choice the expression allows is all permissive."""
    expression = expression.replace("/", " OR ").strip()
    # Small expressions only: "A OR B", "(A OR B) AND C", "A AND B".
    if "AND" in expression:
        terms = re.split(r"\s+AND\s+", expression)
        return all(is_permissive(t.strip().strip("()")) for t in terms)
    return any(i.strip() in PERMISSIVE for i in expression.split(" OR "))


def standard_texts(expression):
    """SPDX fallback texts for an expression: one per id we keep a text for."""
    texts = []
    for licence_id in spdx_ids(expression):
        path = SPDX_TEXTS / f"{licence_id}.txt"
        if path.exists() and path not in texts:
            texts.append(path)
    return texts


def build():
    meta = run_metadata()
    components = []
    for p in linked_packages(meta):
        folder = Path(p["manifest_path"]).parent
        expression = (p.get("license") or "").replace("/", " OR ")
        files = sorted(f for f in folder.iterdir()
                       if f.is_file() and LICENCE_FILE.match(f.name))
        # REUSE layout: one file per licence in LICENSES/.
        if (folder / "LICENSES").is_dir():
            files += sorted(f for f in (folder / "LICENSES").iterdir() if f.is_file())
        files += [folder / rel for rel in EXTRA_FILES.get(p["name"], [])]
        if p.get("license_file"):
            files.append(folder / p["license_file"])
        texts = [(f.relative_to(folder).as_posix(), read_text(f)) for f in files if f.exists()]
        notes = []
        if not texts and expression:
            for path in standard_texts(expression):
                texts.append((f"{path.stem} (standard text)", read_text(path)))
            if texts:
                authors = ", ".join(p.get("authors") or []) or "the authors"
                notes.append(f"No licence file in the package. Copyright {authors}.")
        components.append({
            "name": p["name"],
            "version": p["version"],
            "licence": expression or "unknown",
            "source": p.get("repository") or f"https://crates.io/crates/{p['name']}",
            "texts": texts,
            "notes": notes,
        })

        if p["name"] == "libgit2-sys":
            components.append({
                "name": "libgit2",
                "version": p["version"].split("+")[-1],
                "licence": "GPL-2.0-only WITH libgit2 linking exception; xdiff LGPL-2.1-or-later",
                "source": "https://github.com/libgit2/libgit2",
                "texts": [("COPYING", read_text(folder / "libgit2/COPYING"))],
                "notes": ["Compiled into paw by libgit2-sys. Also contains xdiff, PCRE2, llhttp and sha1dc."],
                "requires": "libgit2",
            })
        if p["name"] == "psl":
            components.append({
                "name": "Public Suffix List",
                "version": "in psl " + p["version"],
                "licence": "MPL-2.0",
                "source": "https://publicsuffix.org/list/",
                "texts": [("MPL-2.0 (standard text)", read_text(SPDX_TEXTS / "MPL-2.0.txt"))],
                "notes": ["The list's data, compiled into the psl crate."],
            })

    toolchain = re.search(r'channel\s*=\s*"([^"]+)"',
                          (ROOT / "rust-toolchain.toml").read_text()).group(1)
    components.append({
        "name": "Rust standard library",
        "version": toolchain,
        "licence": "MIT OR Apache-2.0",
        "source": "https://github.com/rust-lang/rust",
        "texts": [(f"{t.stem} (standard text)", read_text(t)) for t in standard_texts("MIT OR Apache-2.0")],
        "notes": ["Copyright the Rust Project Developers."],
    })

    components.sort(key=lambda c: (c["name"].lower(), c["version"]))
    bodies, index, concerns, missing = [], {}, [], []
    for c in components:
        refs = []
        for title, body in c.pop("texts"):
            if body not in index:
                index[body] = len(bodies)
                bodies.append({"title": title, "body": body})
            refs.append(index[body])
        c["texts"] = refs
        if not refs:
            missing.append(f"{c['name']} {c['version']}")
        requires_key = c.pop("requires", None)
        if requires_key or not is_permissive(c["licence"]):
            key = requires_key or c["licence"]
            template = REQUIRES.get(key, "Not a licence this script knows as permissive: check it by hand.")
            concerns.append({
                "component": f"{c['name']} {c['version']}",
                "licence": c["licence"],
                "requires": template.format(source=c["source"]),
            })
    return {
        "generatedBy": "scripts/licences.py",
        "paw": {"name": "paw", "licence": "MIT", "body": read_text(ROOT / "LICENSE")},
        "components": components,
        "texts": bodies,
        "concerns": concerns,
        "missing": missing,
    }


def serialise(notices):
    return json.dumps(notices, indent=1, ensure_ascii=False, sort_keys=True) + "\n"


def main():
    check = "--check" in sys.argv[1:]
    notices = build()
    data = serialise(notices)
    for line in notices["missing"]:
        print(f"licences: no licence text for {line}", file=sys.stderr)
    if not check:
        OUT.write_text(data, encoding="utf-8")
        print(f"{OUT.relative_to(ROOT)}: {len(notices['components'])} components, "
              f"{len(notices['texts'])} texts, {len(data.encode()) // 1024} KiB")
        return 0

    failed = False
    if notices["missing"]:
        failed = True
    current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
    if current != data:
        failed = True
        print(f"licences: {OUT.relative_to(ROOT)} is stale; run scripts/licences.py", file=sys.stderr)
        # Say which crates differ, so a review can see it at a glance.
        try:
            have = {(c["name"], c["version"]) for c in json.loads(current)["components"]}
        except (ValueError, KeyError):
            have = set()
        want = {(c["name"], c["version"]) for c in notices["components"]}
        for name, version in sorted(want - have):
            print(f"  not covered: {name} {version}", file=sys.stderr)
        for name, version in sorted(have - want):
            print(f"  no longer linked: {name} {version}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
