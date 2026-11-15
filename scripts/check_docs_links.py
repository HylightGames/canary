#!/usr/bin/env python3
"""Fail on relative Markdown links that point at no file.

Covers `docs/**/*.md` plus the repository-root Markdown documents. External
URLs, mail links and pure `#anchor` links are skipped; anchors on relative
links are stripped before the file check (anchor slugs are verified by the
book build, not here). This is the regression gate for the final review's
`CANARY-REL-025`: 55 such links had rotted before it was added.
"""

from __future__ import annotations

import pathlib
import re
import sys

LINK = re.compile(r'\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)')
ROOT_FILES = ("README.md", "CONTRIBUTING.md", "CHANGELOG.md")


def documents() -> list[pathlib.Path]:
    files = sorted(pathlib.Path("docs").rglob("*.md"))
    files.extend(pathlib.Path(name) for name in ROOT_FILES if pathlib.Path(name).is_file())
    return files


def main() -> int:
    broken: list[tuple[str, int, str]] = []
    for path in documents():
        text = path.read_text(encoding="utf-8", errors="replace")
        for match in LINK.finditer(text):
            target = match.group(1).strip()
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            target = target.split("#", 1)[0]
            if not target:
                continue
            if not (path.parent / target).resolve().exists():
                line = text[: match.start()].count("\n") + 1
                broken.append((str(path), line, match.group(1)))

    for path, line, target in broken:
        print(f"{path}:{line}: broken relative link: {target}")
    if broken:
        print(f"\n{len(broken)} broken relative link(s); fix the path or remove the link.")
        return 1
    print(f"checked {len(documents())} Markdown documents: all relative links resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main())
