#!/usr/bin/env python3
"""Rewrite links that leave docs/ into absolute GitHub URLs.

mdBook treats `docs/` as the site root, so a link such as
`../../README.md` resolves inside the repository but 404s on the built
site. This runs before `mdbook build` and rewrites exactly those links.
Anything that resolves inside `docs/` is left untouched, so the
repository keeps its relative links.

Usage: python3 scripts/docs-links.py [docs-dir]
"""

from __future__ import annotations

import os
import re
import sys

OWNER = "HylightGames"
REPO = "canary"
BRANCH = "dev"

LINK = re.compile(r"(!?\[[^\]]*\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")


def is_external(target: str) -> bool:
    return (
        "://" in target
        or target.startswith("#")
        or target.startswith("mailto:")
        or target.startswith("data:")
        or target.startswith("/")
    )


def main() -> int:
    docs = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "docs")
    repo_root = os.path.dirname(docs)
    rewritten = 0
    missing: list[str] = []

    for root, dirs, files in os.walk(docs):
        dirs[:] = [d for d in dirs if d not in {".git", "book"}]
        for name in sorted(files):
            if not name.endswith(".md"):
                continue
            path = os.path.join(root, name)
            with open(path, encoding="utf-8") as handle:
                text = handle.read()

            def replace(match: re.Match[str]) -> str:
                nonlocal rewritten
                start, target, end = match.groups()
                if is_external(target):
                    return match.group(0)

                fragment = ""
                if "#" in target:
                    target, fragment = target.split("#", 1)
                    fragment = "#" + fragment

                resolved = os.path.normpath(os.path.join(os.path.dirname(path), target))
                if resolved.startswith(docs + os.sep):
                    return match.group(0)

                if not os.path.exists(resolved):
                    # Leave broken links alone: a GitHub URL built from a
                    # path that escapes the repository would be invalid.
                    missing.append(f"{os.path.relpath(path, repo_root)} -> {target}")
                    return match.group(0)

                rel = os.path.relpath(resolved, repo_root)
                if os.path.isdir(resolved):
                    url = f"https://github.com/{OWNER}/{REPO}/tree/{BRANCH}/{rel}"
                else:
                    url = f"https://github.com/{OWNER}/{REPO}/blob/{BRANCH}/{rel}"

                rewritten += 1
                return f"{start}{url}{fragment}{end}"

            updated = LINK.sub(replace, text)
            if updated != text:
                with open(path, "w", encoding="utf-8") as handle:
                    handle.write(updated)

    print(f"rewrote {rewritten} outbound link(s)")
    if missing:
        print(f"{len(missing)} target(s) do not exist:", file=sys.stderr)
        for item in missing:
            print(f"  {item}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
