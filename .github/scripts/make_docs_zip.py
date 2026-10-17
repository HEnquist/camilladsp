#!/usr/bin/env python3
"""Build the documentation zip for a release.

Starts at a markdown file (the README), follows every relative link to a local file,
and recursively follows the links in any markdown file found that way.
A link to a directory includes all files in it.
Every file reached is written to the zip with its relative path kept,
so the links between the documents keep working.
A link to a file that does not exist is an error, which makes this double as a link check.
Instruction files and config folders of AI coding tools are never included,
even if they are linked.

Usage: make_docs_zip.py README.md camilladsp-docs.zip
"""

import argparse
import re
import sys
import zipfile
from fnmatch import fnmatch
from pathlib import Path

# Compared in lower case, against the file name and against every folder in the path.
AI_FILES = (
    "agents.md",
    "claude.md",
    "gemini.md",
    "copilot-instructions.md",
    "*.instructions.md",
    ".cursorrules",
    ".windsurfrules",
)
AI_DIRS = (".claude", ".codex", ".cursor", ".gemini", ".windsurf")

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


def is_ai_file(path, root):
    """True for instruction files and config folders of AI coding tools."""
    if any(part.lower() in AI_DIRS for part in path.relative_to(root).parts):
        return True
    return any(fnmatch(path.name.lower(), pattern) for pattern in AI_FILES)


def collect(start):
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
                queue.extend(p for p in sorted(linked.rglob("*")) if p.is_file() and not is_ai_file(p, root))
            elif not is_ai_file(linked, root):
                queue.append(linked)
    return root, sorted(found), errors


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("start", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    root, files, errors = collect(args.start)
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
