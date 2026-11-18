#!/usr/bin/env python3
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

MARKDOWN_FILES = [
    ROOT / "README.md",
    ROOT / "CHANGELOG.md",
    ROOT / "SECURITY.md",
    ROOT / "SUMMARY.md",
    *sorted((ROOT / "docs").glob("*.md")),
    ROOT / "extension" / "README.md",
]
RUST_FILES = sorted((ROOT / "src").glob("*.rs"))

BANNED_WORDS = {
    "powerful", "robust", "seamless", "flexible", "scalable", "efficient",
    "modern", "advanced", "lightweight", "easy", "easily", "simple", "simply",
    "intuitive", "comprehensive", "stable", "generic", "trusted", "common",
    "normal", "relevant", "generally", "usually", "typically", "basically",
    "just", "actually", "currently", "probably", "recommended", "preferably",
    "might", "could", "should", "may", "can", "safe", "safely", "secure",
    "securely", "proper", "obvious", "straightforward", "convenient", "optimal",
    "ideal",
}

BANNED_PHRASES = {
    "best practice", "state of the art", "production ready", "enterprise ready",
    "in order to", "note that", "it is important", "remember that",
    "in other words", "as mentioned", "as described", "as shown",
    "this means", "this ensures", "this allows", "this provides",
    "this makes", "this helps", "can be used to", "designed to",
    "intended to", "where possible", "when appropriate", "as needed",
    "if needed", "there is", "there are", "in terms of", "with regard to",
    "with respect to", "for the purpose of", "due to the fact that",
    "a number of", "various", "and so on",
}

SOURCE_BANNED = {
    "powerful", "robust", "seamless", "scalable", "efficient", "modern",
    "advanced", "lightweight", "easy", "easily", "intuitive", "comprehensive",
    "generally", "usually", "typically", "basically", "actually", "probably",
}

MAX_SENTENCE_WORDS = 30
MAX_PARAGRAPH_WORDS = 80

INLINE_CODE = re.compile(r"\x60[^\x60]*\x60")
LINK_URL = re.compile(r"\]\([^)]*\)")
WORD = re.compile(r"[A-Za-z][A-Za-z'-]*")
SENTENCE = re.compile(r"(?<=[.!?])\s+")
PASSIVE = re.compile(
    r"\b(?:is|are|was|were|be|been|being)\s+"
    r"(?:used|designed|intended|handled|performed|processed|managed|provided|"
    r"supported|allowed|required|configured|generated|created|stored|read|written|"
    r"checked|verified|validated|returned|exposed|blocked|rejected|preserved|"
    r"retained|applied|copied|reported|normalized|admitted|hashed|reparsed)\b",
    re.IGNORECASE,
)
FENCE = chr(96) * 3


def clean_prose(text: str) -> str:
    text = INLINE_CODE.sub("", text)
    text = LINK_URL.sub("]", text)
    return re.sub(r"[*_~]", "", text)


def normalized(text: str) -> str:
    return re.sub(r"\s+", " ", clean_prose(text)).strip().lower()


def check_terms(path: Path, line_no: int, text: str, errors: list[str]) -> None:
    clean = normalized(text)
    if not clean:
        return

    for phrase in sorted(BANNED_PHRASES):
        if phrase in clean:
            errors.append(f"{path.relative_to(ROOT)}:{line_no}: banned phrase: {phrase!r}")

    words = set(WORD.findall(clean))
    for word in sorted(BANNED_WORDS & words):
        errors.append(f"{path.relative_to(ROOT)}:{line_no}: banned word: {word!r}")

    match = PASSIVE.search(clean)
    if match:
        errors.append(
            f"{path.relative_to(ROOT)}:{line_no}: passive construction: {match.group(0)!r}"
        )


def check_sentence_lengths(path: Path, line_no: int, paragraph: str, errors: list[str]) -> None:
    clean = clean_prose(paragraph)
    words = WORD.findall(clean)
    if len(words) > MAX_PARAGRAPH_WORDS:
        errors.append(
            f"{path.relative_to(ROOT)}:{line_no}: paragraph has {len(words)} words; "
            f"limit is {MAX_PARAGRAPH_WORDS}"
        )
    for sentence in SENTENCE.split(clean):
        count = len(WORD.findall(sentence))
        if count > MAX_SENTENCE_WORDS:
            errors.append(
                f"{path.relative_to(ROOT)}:{line_no}: sentence has {count} words; "
                f"limit is {MAX_SENTENCE_WORDS}"
            )


def check_markdown(path: Path, errors: list[str]) -> None:
    lines = path.read_text(encoding="utf-8").splitlines()
    in_fence = False
    paragraph: list[str] = []
    paragraph_start = 0

    def flush() -> None:
        nonlocal paragraph
        if paragraph:
            check_sentence_lengths(path, paragraph_start, " ".join(paragraph), errors)
            paragraph = []

    for line_no, line in enumerate(lines, 1):
        stripped = line.strip()

        if stripped.startswith(FENCE):
            flush()
            in_fence = not in_fence
            continue
        if in_fence:
            continue

        if stripped.startswith("|"):
            for cell in stripped.strip("|").split("|"):
                check_terms(path, line_no, cell, errors)
            flush()
            continue

        if not stripped:
            flush()
            continue

        check_terms(path, line_no, stripped, errors)

        if stripped.startswith("#") or stripped.startswith("- ") or re.match(r"^\d+\.\s", stripped):
            flush()
            check_sentence_lengths(path, line_no, stripped.lstrip("#- 0123456789."), errors)
            continue

        if not paragraph:
            paragraph_start = line_no
        paragraph.append(stripped)

    flush()


def check_rust(path: Path, errors: list[str]) -> None:
    lines = path.read_text(encoding="utf-8").splitlines()
    paragraph: list[str] = []
    paragraph_start = 0

    def flush() -> None:
        nonlocal paragraph
        if paragraph:
            check_sentence_lengths(path, paragraph_start, " ".join(paragraph), errors)
            paragraph = []

    for line_no, line in enumerate(lines, 1):
        stripped = line.lstrip()
        doc: str | None = None
        if stripped.startswith("//!"):
            doc = stripped[3:].strip()
        elif stripped.startswith("///"):
            doc = stripped[3:].strip()

        if doc is not None:
            if not doc:
                flush()
                continue
            check_terms(path, line_no, doc, errors)
            if doc.startswith("#") or doc.startswith("* ") or doc.startswith("- "):
                flush()
                check_sentence_lengths(
                    path,
                    line_no,
                    doc.lstrip("#*- "),
                    errors,
                )
                continue
            if not paragraph:
                paragraph_start = line_no
            paragraph.append(doc)
            continue

        flush()

        if "description =" in line or "#[command(about =" in line:
            check_terms(path, line_no, line, errors)

    flush()

    raw = path.read_text(encoding="utf-8").lower()
    for word in sorted(SOURCE_BANNED):
        pattern = re.compile(rf"\b{re.escape(word)}\b")
        for line_no, line in enumerate(raw.splitlines(), 1):
            if pattern.search(line):
                errors.append(
                    f"{path.relative_to(ROOT)}:{line_no}: banned source word: {word!r}"
                )


def main() -> int:
    errors: list[str] = []
    for path in MARKDOWN_FILES:
        check_markdown(path, errors)
    for path in RUST_FILES:
        check_rust(path, errors)

    if errors:
        print("Technical language check failed:", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        return 1

    print("Technical language check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
