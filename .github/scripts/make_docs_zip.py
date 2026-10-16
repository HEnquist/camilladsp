#!/usr/bin/env python3
"""Build the documentation zip for a release.

Starts at a markdown file (the README), follows every relative link to a local file,
and recursively follows the links in any markdown file found that way.
A link to a directory includes all files in it.
Every file reached is written to the zip with its relative path kept,
so the links between the documents keep working.
A link to a file that does not exist is an error, which makes this double as a link check.

Usage: make_docs_zip.py README.md camilladsp-docs.zip [--exclude AGENTS.md ...]
"""

import argparse
import re
import sys
import zipfile
from pathlib import Path

FENCE = re.compile(r"^(```|~~~).*?^\1", re.S | re.M)
# [text](target), [text]: target, and <img src="target">
INLINE = re.compile(r"\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
REFERENCE = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*<?(\S+?)>?(?:\s|$)", re.M)
HTML_SRC = re.compile(r"""(?:src|href)\s*=\s*["']([^"']+)["']""")
EXTERNAL = re.compile(r"^([a-zA-Z][a-zA-Z0-9+.-]*:|//|#)")


def local_targets(text):
    text = FENCE.sub("", text)
    for pattern in (INLINE, REFERENCE, HTML_SRC):
        for match in pattern.finditer(text):
            target = match.group(1)
            if EXTERNAL.match(target):
                continue
            target = re.split(r"[#?]", target, maxsplit=1)[0]
            if target:
                yield target


def collect(start, excluded):
    root = start.parent.resolve()
    found = {}
    errors = []
    queue = [start.resolve()]
    while queue:
        path = queue.pop()
        if path in found:
            continue
        found[path] = None
        if path.suffix.lower() != ".md":
            continue
        for target in local_targets(path.read_text(encoding="utf-8")):
            linked = (path.parent / target).resolve()
            if root not in linked.parents and linked != root:
                errors.append(f"{path.relative_to(root)}: link leaves the repo: {target}")
            elif not linked.exists():
                errors.append(f"{path.relative_to(root)}: broken link: {target}")
            elif linked.is_dir():
                queue.extend(p for p in sorted(linked.rglob("*")) if p.is_file())
            elif linked.relative_to(root).as_posix() not in excluded:
                queue.append(linked)
    return root, sorted(found), errors


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("start", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--exclude", nargs="*", default=[], help="paths relative to the start file")
    args = parser.parse_args()

    root, files, errors = collect(args.start, set(args.exclude))
    if errors:
        print("\n".join(errors), file=sys.stderr)
        sys.exit(1)
    with zipfile.ZipFile(args.output, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in files:
            name = path.relative_to(root).as_posix()
            archive.write(path, f"camilladsp-docs/{name}")
            print(name)


if __name__ == "__main__":
    main()
