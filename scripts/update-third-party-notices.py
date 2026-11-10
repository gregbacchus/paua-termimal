#!/usr/bin/env python3
"""Generate THIRD-PARTY-NOTICES.md: the licences of everything Paua ships inside its binary.

  python3 scripts/update-third-party-notices.py          # (re)write THIRD-PARTY-NOTICES.md
  python3 scripts/update-third-party-notices.py --check  # exit 1 if the file is out of date

What goes in:
  * every Rust crate the binary is built from (the normal dependency graph for Linux; dev and
    build-script-only crates are not linked into the binary), with its licence text,
  * libghostty-vt, which is linked in statically: Ghostty's licence and the licences of the
    components bundled in it (simdutf, Highway, uucode, the Zig runtime).

Needs a normal build first (`cargo build`): Ghostty's source is fetched into `target/` by
libghostty-vt-sys, and its Zig packages into Zig's cache (`~/.cache/zig`, or $ZIG_GLOBAL_CACHE_DIR).
Run it again whenever dependencies change (Cargo.toml / Cargo.lock) or libghostty-vt is updated.
The output is deterministic, so a clean run on an unchanged tree changes nothing.
"""

import glob
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD-PARTY-NOTICES.md"
TARGET = "x86_64-unknown-linux-gnu"
LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "UNLICENSE", "NOTICE", "COPYRIGHT")


def fail(msg):
    sys.exit(f"update-third-party-notices: {msg}")


def read_text(path):
    return path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").strip("\n")


def license_files(directory):
    """Top-level licence-like files of a package directory, in a stable order."""
    files = []
    for entry in sorted(directory.iterdir()):
        if entry.is_file() and entry.name.upper().startswith(LICENSE_PREFIXES):
            if entry.suffix.lower() in (".rs", ".toml", ".cfg", ".py", ".sh", ".json"):
                continue
            files.append(entry)
    return files


# ---------------------------------------------------------------------------------------------
# Rust crates
# ---------------------------------------------------------------------------------------------


def runtime_crates():
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
            cwd=ROOT,
        )
    )
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            # kind None = a normal dependency (linked in); "build" / "dev" are not shipped.
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    seen.discard(root)
    return sorted((packages[p] for p in seen), key=lambda p: (p["name"], p["version"]))


def crate_entries(crates):
    """(licence expression, [(name, version, repository)], text) grouped by identical text."""
    groups = {}  # text hash -> {"text": str, "crates": [...], "spdx": set}
    missing = []
    for crate in crates:
        directory = Path(crate["manifest_path"]).parent
        spdx = crate.get("license") or "see licence file"
        texts = [(f.name, read_text(f)) for f in license_files(directory)]
        if not texts:
            missing.append(f'{crate["name"]} {crate["version"]} ({spdx})')
            texts = [("", f"(No licence file is included in this crate's package; it is licensed under: {spdx}.)")]
        for _name, text in texts:
            key = hashlib.sha256(text.encode()).hexdigest()
            g = groups.setdefault(key, {"text": text, "crates": [], "spdx": set()})
            entry = (crate["name"], crate["version"], crate.get("repository") or "")
            if entry not in g["crates"]:
                g["crates"].append(entry)
            g["spdx"].add(spdx)
    return groups, missing


# ---------------------------------------------------------------------------------------------
# libghostty-vt and what is bundled in it
# ---------------------------------------------------------------------------------------------


def ghostty_source():
    env = os.environ.get("GHOSTTY_SOURCE_DIR")
    candidates = [Path(env)] if env else []
    pattern = str(ROOT / "target" / "*" / "build" / "libghostty-vt-sys-*" / "out" / "ghostty-src")
    candidates += sorted((Path(p) for p in glob.glob(pattern)), key=lambda p: p.stat().st_mtime, reverse=True)
    for c in candidates:
        if (c / "LICENSE").is_file():
            return c
    fail("Ghostty's source was not found under target/. Run `cargo build` first (or set GHOSTTY_SOURCE_DIR).")


def zig_cache_packages():
    cache = Path(os.environ.get("ZIG_GLOBAL_CACHE_DIR") or Path.home() / ".cache" / "zig") / "p"
    return sorted(p for p in cache.glob("*") if p.is_dir()) if cache.is_dir() else []


def find_package(packages, what, test):
    for p in packages:
        try:
            if test(p):
                return p
        except OSError:
            continue
    fail(f"the Zig package for {what} was not found in Zig's cache ({'/'.join(['~/.cache/zig', 'p'])}). Run `cargo build` first.")


def embedded_notice(path, first_line_marker):
    """A notice embedded in a source file: from the comment start before the marker to its `*/`."""
    lines = read_text(path).split("\n")
    for i, line in enumerate(lines):
        if first_line_marker in line:
            start = i
            while start > 0 and not lines[start].lstrip().startswith("/*"):
                start -= 1
            end = i
            while end < len(lines) and lines[end].strip() != "*/":
                end += 1
            return "\n".join(lines[start : end + 1])
    fail(f"the notice containing {first_line_marker!r} was not found in {path}")


def ghostty_entries():
    """(name, url, licence, note, [(label, text)]) for libghostty-vt and what is bundled in it."""
    src = ghostty_source()
    packages = zig_cache_packages()
    components = [
        ("Ghostty (libghostty-vt)", "https://github.com/ghostty-org/ghostty", "MIT", "", [(f.name, read_text(f)) for f in [src / "LICENSE"]]),
    ]

    # simdutf is vendored as a single amalgamated source file without a licence file of its own: it is
    # dual licensed (Apache-2.0 OR MIT; see its repository), and the amalgamation embeds one more
    # notice, reproduced here.
    simdutf_header = src / "pkg" / "simdutf" / "vendor" / "simdutf.h"
    if not simdutf_header.is_file():
        fail("pkg/simdutf/vendor/simdutf.h was not found in Ghostty's source")
    components.append(
        (
            "simdutf",
            "https://github.com/simdutf/simdutf",
            "Apache-2.0 OR MIT",
            "Licence texts: https://github.com/simdutf/simdutf/blob/master/LICENSE-MIT and LICENSE-APACHE. "
            "The source file also embeds this notice (isadetection.h):",
            [("embedded notice", embedded_notice(simdutf_header, "Facebook, Inc            (Adam Paszke)"))],
        )
    )
    highway = find_package(packages, "Highway", lambda p: (p / "hwy").is_dir() and (p / "LICENSE").is_file())
    components.append(("Highway", "https://github.com/google/highway", "Apache-2.0 OR BSD-3-Clause", "", [(f.name, read_text(f)) for f in license_files(highway)]))
    uucode = find_package(packages, "uucode", lambda p: p.name.startswith("uucode-") and any(p.glob("LICENSE*")))
    components.append(("uucode", "https://github.com/jacobsandlund/uucode", "MIT", "", [(f.name, read_text(f)) for f in license_files(uucode)]))

    zig_dir = Path(os.environ.get("ZIG_HOME") or Path.home() / ".local" / "zig")
    zig_license = zig_dir / "LICENSE"
    if not zig_license.is_file():
        zig_which = subprocess.run(["which", "zig"], capture_output=True, text=True).stdout.strip()
        if zig_which:
            zig_license = Path(zig_which).resolve().parent / "LICENSE"
    if not zig_license.is_file():
        fail("Zig's LICENSE file was not found next to the zig binary (set ZIG_HOME to its directory).")
    components.append(("Zig runtime (compiler-rt, standard library)", "https://ziglang.org", "MIT", "", [(zig_license.name, read_text(zig_license))]))
    return components


# ---------------------------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------------------------

HEADER = """# Third-party notices

Paua itself is MIT licensed (see [LICENSE](LICENSE)). The Paua binary contains the following
third-party software, whose licences are reproduced below as they require.

This file is generated: run `python3 scripts/update-third-party-notices.py` after changing
dependencies (Cargo.toml / Cargo.lock) or updating libghostty-vt, and commit the result.

Not part of the binary, and not listed here: GTK 4, libadwaita and the other system libraries Paua
links dynamically (installed from your distribution under their own licences, LGPL for GTK and
libadwaita), and the Nerd Fonts and colour themes that `paua fonts` / `paua themes` download on
request (each keeps the licence it was published under, installed beside the files).

"""


def render():
    crates = runtime_crates()
    groups, missing = crate_entries(crates)
    components = ghostty_entries()

    out = [HEADER]
    out.append("## libghostty-vt (statically linked)\n")
    out.append(
        "The terminal engine is libghostty-vt from the Ghostty project, built from source by the "
        "`libghostty-vt-sys` crate. It bundles the components below.\n"
    )
    for name, url, spdx, note, texts in components:
        out.append(f"### {name}\n")
        out.append(f"{url}  \nLicence: {spdx}\n")
        if note:
            out.append(note + "\n")
        for label, text in texts:
            out.append(f"{label}:\n")
            out.append("```text\n" + text + "\n```\n")

    out.append("## Rust crates\n")
    out.append(f"{len(crates)} crates are compiled into the binary. Crates sharing a licence text are listed together.\n")
    ordered = sorted(groups.values(), key=lambda g: (sorted(g["spdx"]), g["crates"][0][0], g["crates"][0][1]))
    for n, g in enumerate(ordered, 1):
        spdx = " / ".join(sorted(g["spdx"]))
        out.append(f"### Licence text {n}: {spdx}\n")
        out.append("Used by:\n")
        for name, version, repo in g["crates"]:
            suffix = f" ({repo})" if repo else ""
            out.append(f"- {name} {version}{suffix}")
        out.append("\n```text\n" + g["text"] + "\n```\n")
    text = "\n".join(out).rstrip("\n") + "\n"
    return text, missing


def main():
    check = "--check" in sys.argv[1:]
    text, missing = render()
    if missing:
        print("note: these crates ship no licence file (their SPDX expression is recorded instead):", file=sys.stderr)
        for m in missing:
            print(f"  - {m}", file=sys.stderr)
    if check:
        current = OUT.read_text(encoding="utf-8") if OUT.is_file() else ""
        if current != text:
            fail("THIRD-PARTY-NOTICES.md is out of date; run `python3 scripts/update-third-party-notices.py`.")
        print("THIRD-PARTY-NOTICES.md is up to date.")
        return
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)} ({len(text) // 1024} KiB)")


if __name__ == "__main__":
    main()
