#!/usr/bin/env python3
"""check-nsis-translations.py

Fail-loud guard against "empty Hebrew label" bugs in the installer/uninstaller.

What it checks
--------------
1. Parses `src-tauri/nsis/Hebrew.nsh` and collects every string name that
   has a Hebrew `LangString` translation.

2. Parses the generated `src-tauri/target/release/nsis/**/installer.nsi`
   (produced by `tauri build`) and collects every `$(name)` reference.

3. Fails if any reference has no matching Hebrew translation.

NSIS's `LangString` table is shared between the installer and the
uninstaller, so a single `LangString NAME ${LANG_HEBREW} "..."` entry
covers any `$(NAME)` use inside either side of the wizard (including
`Function un.*` blocks like the "Delete app data" checkbox on the
uninstall Confirm page).

Note: NSIS 3.x does NOT have `UninstallLangString`. Attempting to use it
makes makensis abort with `Invalid command: UninstallLangString`.

Usage
-----
    python scripts/check-nsis-translations.py

Run after `npm run tauri build`. Exits 0 on success, 1 on missing strings.
If the generated `installer.nsi` is absent (no build yet) the script
exits 0 with a note, so it is safe to run on a fresh checkout.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
HEB_SRC = REPO_ROOT / "src-tauri" / "nsis" / "Hebrew.nsh"
GENERATED_NSIS_DIR = REPO_ROOT / "src-tauri" / "target" / "release" / "nsis"

# NSIS resolves these internally (no translation needed from us).
NSIS_BUILTINS = {"^Name", "^NameDA", "^Version", "^Caption", "^SetupCaption"}


def collect_hebrew_translations() -> set[str]:
    if not HEB_SRC.exists():
        print(f"ERROR: {HEB_SRC} not found", file=sys.stderr)
        sys.exit(2)
    text = HEB_SRC.read_text(encoding="utf-8-sig")  # tolerate BOM
    return set(re.findall(r"LangString\s+(\w+)\s+\$\{LANG_HEBREW\}", text))


def collect_references() -> set[str] | None:
    candidates = list(GENERATED_NSIS_DIR.rglob("installer.nsi"))
    if not candidates:
        return None
    text = candidates[0].read_text(encoding="utf-8", errors="replace")
    refs = set()
    for match in re.findall(r"\$\(([a-zA-Z_][a-zA-Z0-9_.]*)\)", text):
        if match in NSIS_BUILTINS or match.startswith("^"):
            continue
        refs.add(match)
    return refs


def main() -> int:
    translated = collect_hebrew_translations()
    refs = collect_references()

    if refs is None:
        print(
            "note: no generated installer.nsi found — run `npm run tauri build` "
            f"first for the cross-check. Source-file translation count: "
            f"{len(translated)} strings."
        )
        return 0

    missing = sorted(refs - translated)
    if missing:
        for name in missing:
            print(
                f"ERROR: $({name}) is referenced by the generated NSIS script "
                "but has no `LangString ${LANG_HEBREW}` entry in "
                "src-tauri/nsis/Hebrew.nsh",
                file=sys.stderr,
            )
        print(
            f"\n{len(missing)} missing NSIS Hebrew translation(s). "
            "Add `LangString <name> ${LANG_HEBREW} \"...\"` for each in Hebrew.nsh.",
            file=sys.stderr,
        )
        return 1

    print(
        f"OK: Hebrew.nsh translates all {len(refs)} `$(...)` references "
        "used by the built installer."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
