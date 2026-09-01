#!/usr/bin/env python3
"""Fail when generated documentation contains a broken internal URL or anchor."""

from __future__ import annotations

import argparse
import html.parser
import pathlib
import sys
import urllib.parse


class Document(html.parser.HTMLParser):
    """Collect link-bearing attributes and local anchor identities."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.links: list[str] = []
        self.anchors: set[str] = set()

    def handle_starttag(
        self, tag: str, attrs: list[tuple[str, str | None]]
    ) -> None:
        del tag
        attributes = dict(attrs)
        for key in ("id", "name"):
            if attributes.get(key):
                self.anchors.add(attributes[key] or "")
        for key in ("href", "src"):
            if attributes.get(key):
                self.links.append(attributes[key] or "")


def parse_document(path: pathlib.Path) -> Document:
    document = Document()
    document.feed(path.read_text(encoding="utf-8"))
    document.close()
    return document


def local_target(
    root: pathlib.Path, source: pathlib.Path, raw_url: str, base: str
) -> tuple[pathlib.Path, str] | None:
    url = urllib.parse.urlsplit(raw_url)
    if url.scheme or url.netloc:
        return None
    decoded = urllib.parse.unquote(url.path)
    if not decoded:
        target = source
    elif decoded.startswith(base):
        target = root / decoded[len(base) :]
    elif decoded.startswith("/"):
        raise ValueError(f"root-relative URL escapes configured base {base!r}")
    else:
        target = source.parent / decoded
    if decoded.endswith("/") or target.is_dir() or not target.suffix:
        target = target / "index.html"
    target = target.resolve(strict=False)
    try:
        target.relative_to(root)
    except ValueError as error:
        raise ValueError("URL escapes generated documentation root") from error
    return target, urllib.parse.unquote(url.fragment)


def check(directory: pathlib.Path, base: str) -> tuple[int, list[str]]:
    root = directory.resolve(strict=True)
    if not root.is_dir():
        raise ValueError(f"documentation output is not a directory: {root}")
    if not base.startswith("/") or not base.endswith("/") or "//" in base:
        raise ValueError("base must be one absolute path prefix ending in '/'")

    documents = {
        path.resolve(): parse_document(path) for path in sorted(root.rglob("*.html"))
    }
    if not documents:
        raise ValueError(f"documentation output contains no HTML pages: {root}")

    checked = 0
    failures: list[str] = []
    for source, document in documents.items():
        for raw_url in document.links:
            checked += 1
            try:
                resolved = local_target(root, source, raw_url, base)
            except ValueError as error:
                failures.append(f"{source.relative_to(root)}: {raw_url!r}: {error}")
                continue
            if resolved is None:
                continue
            target, fragment = resolved
            if not target.is_file():
                failures.append(
                    f"{source.relative_to(root)}: {raw_url!r}: target is missing"
                )
                continue
            if fragment and target.suffix.lower() == ".html":
                target_document = documents.get(target)
                if target_document is None:
                    target_document = parse_document(target)
                    documents[target] = target_document
                if fragment not in target_document.anchors:
                    failures.append(
                        f"{source.relative_to(root)}: {raw_url!r}: anchor is missing"
                    )
    return checked, failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True, type=pathlib.Path)
    parser.add_argument("--base", required=True)
    args = parser.parse_args()
    try:
        checked, failures = check(args.directory, args.base)
    except (OSError, UnicodeError, ValueError) as error:
        print(f"documentation link gate failed: {error}", file=sys.stderr)
        return 1
    if failures:
        for failure in failures:
            print(f"broken documentation link: {failure}", file=sys.stderr)
        print(
            f"documentation link gate found {len(failures)} failure(s)",
            file=sys.stderr,
        )
        return 1
    print(f"checked {checked} generated documentation links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
