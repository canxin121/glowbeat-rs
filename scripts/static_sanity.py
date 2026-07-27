#!/usr/bin/env python3
"""Dependency-free structural and cross-language ABI checks.

This does not replace `cargo check`; it catches packaging mistakes and interface
skew in environments where a Rust toolchain is unavailable.
"""
from __future__ import annotations

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
OPEN = {'(': ')', '[': ']', '{': '}'}
CLOSE = {v: k for k, v in OPEN.items()}


def check_delimiters(path: Path) -> list[str]:
    text = path.read_text(encoding='utf-8')
    errors: list[str] = []
    stack: list[tuple[str, int, int]] = []
    i = 0
    line = 1
    col = 1
    block_depth = 0
    line_comment = False
    string_quote: str | None = None
    raw_hashes: int | None = None
    char_literal = False
    escape = False
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ''
        if ch == '\n':
            line += 1
            col = 0
            line_comment = False
        if line_comment:
            i += 1; col += 1; continue
        if block_depth:
            if ch == '/' and nxt == '*':
                block_depth += 1; i += 2; col += 2; continue
            if ch == '*' and nxt == '/':
                block_depth -= 1; i += 2; col += 2; continue
            i += 1; col += 1; continue
        if raw_hashes is not None:
            if ch == '"' and text.startswith('#' * raw_hashes, i + 1):
                i += 1 + raw_hashes
                col += 1 + raw_hashes
                raw_hashes = None
            i += 1; col += 1; continue
        if string_quote is not None or char_literal:
            if escape:
                escape = False
            elif ch == '\\':
                escape = True
            elif string_quote is not None and ch == string_quote:
                string_quote = None
            elif char_literal and ch == "'":
                char_literal = False
            i += 1; col += 1; continue
        if ch == '/' and nxt == '/':
            line_comment = True; i += 2; col += 2; continue
        if ch == '/' and nxt == '*':
            block_depth = 1; i += 2; col += 2; continue
        if ch == 'r':
            match = re.match(r'r(#{0,16})"', text[i:])
            if match:
                raw_hashes = len(match.group(1))
                consumed = len(match.group(0))
                i += consumed; col += consumed; continue
        if ch == '"':
            string_quote = '"'; i += 1; col += 1; continue
        # Character literal. Lifetimes such as `'a` do not have a closing quote.
        if ch == "'" and i + 2 < len(text) and text[i + 2] == "'":
            char_literal = True; i += 1; col += 1; continue
        if ch in OPEN:
            stack.append((ch, line, col))
        elif ch in CLOSE:
            if not stack or stack[-1][0] != CLOSE[ch]:
                errors.append(f'{path.relative_to(ROOT)}:{line}:{col}: unmatched {ch}')
            else:
                stack.pop()
        i += 1; col += 1
    if block_depth:
        errors.append(f'{path.relative_to(ROOT)}: unterminated block comment')
    if string_quote is not None or raw_hashes is not None or char_literal:
        errors.append(f'{path.relative_to(ROOT)}: unterminated literal')
    for token, token_line, token_col in stack:
        errors.append(f'{path.relative_to(ROOT)}:{token_line}:{token_col}: unmatched {token}')
    return errors


def exported_rust_symbols(text: str) -> set[str]:
    return set(re.findall(
        r'#\s*\[\s*no_mangle\s*\]\s*pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+(glowbeat_[A-Za-z0-9_]+)',
        text,
        flags=re.S,
    ))


def header_symbols(text: str) -> set[str]:
    return set(re.findall(r'\b(glowbeat_[A-Za-z0-9_]+)\s*\(', text))


def csharp_symbols(text: str) -> set[str]:
    return set(re.findall(r'extern\s+[^;\n]+\s+(glowbeat_[A-Za-z0-9_]+)\s*\(', text))


def main() -> int:
    errors: list[str] = []
    rust_files = sorted(ROOT.rglob('*.rs'))
    for path in rust_files:
        if 'target' not in path.parts:
            errors.extend(check_delimiters(path))
    required = [
        'src/lib.rs', 'src/director.rs', 'src/offline.rs', 'src/score.rs',
        'src/streaming.rs', 'src/live.rs', 'src/ffi.rs', 'include/glowbeat.h', 'README.md',
        '.github/workflows/ci.yml', 'bindings/swift/GlowBeat.swift',
        'bindings/csharp/GlowBeatNative.cs', 'bindings/csharp/GlowBeat.csproj',
        'assets/demo_124bpm.wav',
        'examples/c_api_smoke.c', 'scripts/validate_links.py',
    ]
    for relative in required:
        if not (ROOT / relative).is_file():
            errors.append(f'missing required file: {relative}')
    forbidden = re.compile(r'\b(todo!|unimplemented!|unreachable!)\s*\(')
    for path in rust_files:
        text = path.read_text(encoding='utf-8')
        for match in forbidden.finditer(text):
            line = text.count('\n', 0, match.start()) + 1
            errors.append(f'{path.relative_to(ROOT)}:{line}: forbidden placeholder {match.group(1)}')

    rust = exported_rust_symbols((ROOT / 'src/ffi.rs').read_text())
    header = header_symbols((ROOT / 'include/glowbeat.h').read_text())
    csharp = csharp_symbols((ROOT / 'bindings/csharp/GlowBeatNative.cs').read_text())
    for name, symbols in [('C header', header), ('C# binding', csharp)]:
        missing = sorted(rust - symbols)
        extra = sorted(symbols - rust)
        if missing:
            errors.append(f'{name} is missing Rust exports: {", ".join(missing)}')
        if extra:
            errors.append(f'{name} declares unknown exports: {", ".join(extra)}')

    if errors:
        print('\n'.join(errors), file=sys.stderr)
        return 1
    print(
        f'structural sanity passed for {len(rust_files)} Rust files; '
        f'{len(rust)} FFI exports agree across Rust, C, and C#'
    )
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
