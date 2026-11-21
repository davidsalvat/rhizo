#!/usr/bin/env python3
"""Checks relative links in the repository's Markdown files (change `faster-ci`).

    python3 .github/scripts/check-links.py [file.md ...]

Without files it checks every Markdown file git tracks. A link or image
whose target is a path (not a URL, not only an anchor) must name a file or
directory that exists; links inside code are ignored. External URLs are not
fetched: their state is outside the pull request. Exits 1 on a broken link
and prints each as `file:line: target`.
"""

import os
import re
import subprocess
import sys

LINK = re.compile(r"!?\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
REFERENCE = re.compile(r"^\s{0,3}\[(?!\^)[^\]]+\]:\s*<?(\S+?)>?(?:\s|$)")
HTML = re.compile(r"""<(?:img|a|source)\b[^>]*?\b(?:src|href|srcset)=["']([^"']+)["']""")
INLINE_CODE = re.compile(r"`[^`]*`")


def targets(line):
    line = INLINE_CODE.sub("", line)
    for pattern in (LINK, REFERENCE, HTML):
        for match in pattern.finditer(line):
            yield match.group(1)


def is_path(target):
    return not (
        target.startswith("#")
        or re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target)
        or target.startswith("//")
    )


def broken(root, path):
    found = []
    fence = None
    with open(os.path.join(root, path), encoding="utf-8") as f:
        for number, line in enumerate(f, 1):
            stripped = line.lstrip()
            if stripped.startswith(("```", "~~~")):
                marker = stripped[:3]
                fence = None if fence == marker else (fence or marker)
                continue
            if fence:
                continue
            for target in targets(line):
                if not is_path(target):
                    continue
                file = target.split("#", 1)[0].split("?", 1)[0]
                if not file:
                    continue
                base = root if file.startswith("/") else os.path.dirname(os.path.join(root, path))
                if not os.path.exists(os.path.normpath(os.path.join(base, file.lstrip("/")))):
                    found.append(f"{path}:{number}: {target}")
    return found


def main(args):
    root = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True
    ).stdout.strip()
    files = args or subprocess.run(
        ["git", "ls-files", "*.md"], cwd=root, capture_output=True, text=True, check=True
    ).stdout.split()
    errors = [e for path in files for e in broken(root, path)]
    for error in errors:
        print(error)
    print(f"{len(files)} Markdown files checked, {len(errors)} broken links")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
