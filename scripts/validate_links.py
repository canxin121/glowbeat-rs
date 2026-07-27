#!/usr/bin/env python3
"""Validate repository-local Markdown links without external dependencies."""
from __future__ import annotations

from pathlib import Path
from urllib.parse import unquote
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
LINK = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
SKIP_PREFIXES = ("http://", "https://", "mailto:", "tel:", "data:", "#", "sandbox:")


def main() -> int:
    errors: list[str] = []
    checked = 0
    for document in sorted(ROOT.rglob("*.md")):
        if any(part in {"target", ".git"} for part in document.parts):
            continue
        text = document.read_text(encoding="utf-8")
        for match in LINK.finditer(text):
            raw = match.group(1).strip()
            # Optional Markdown title: path "title".
            if raw.startswith("<") and ">" in raw:
                raw = raw[1:raw.index(">")]
            elif " \"" in raw:
                raw = raw.split(" \"", 1)[0]
            target = unquote(raw.split("#", 1)[0].strip())
            if not target or target.startswith(SKIP_PREFIXES):
                continue
            checked += 1
            resolved = (document.parent / target).resolve()
            try:
                resolved.relative_to(ROOT.resolve())
            except ValueError:
                errors.append(f"{document.relative_to(ROOT)}: link escapes repository: {raw}")
                continue
            if not resolved.exists():
                line = text.count("\n", 0, match.start()) + 1
                errors.append(
                    f"{document.relative_to(ROOT)}:{line}: missing local link target: {raw}"
                )
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"markdown link validation passed: {checked} local links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
