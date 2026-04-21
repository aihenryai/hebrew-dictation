#!/usr/bin/env python3
"""check-nsis-translations.py

Fail-loud guard against "empty Hebrew checkbox" bugs in the installer.

What it checks
--------------
1. Parses `src-tauri/nsis/Hebrew.nsh` and collects every string name that
   has a Hebrew translation — for the installer table, the uninstaller
   table, or both (via our `TauriLangString` macro).

2. Parses the generated `src-tauri/target/release/nsis/**/installer.nsi`
   (produced by `tauri build`). For every `$(name)` reference, it tracks
   whether that reference lives inside a `Function un.*` / `Section un.*`
   block (uninstaller scope) or outside (installer scope).

3. Fails if any reference resolves to the Hebrew language but the
   corresponding `LangString` (installer) or `UninstallLangString`
   (uninstaller) is missing.

This is the root-cause safeguard for the v2.2.2 bug where the "Delete
app data" checkbox on the uninstall Confirm page had an empty label: the
source Hebrew.nsh only populated the installer string table, so
`$(deleteAppData)` inside `un.ConfirmShow` fell back to an empty string.

Usage
-----
    python scripts/check-nsis-translations.py

Run after `npm run tauri build`. Exits 0 on success, 1 on missing strings.
If `installer.nsi` has not been generated yet (no build ran) the cross-
check against the generated file is skipped with a note.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
HEB_SRC = REPO_ROOT / "src-tauri" / "nsis" / "Hebrew.nsh"
GENERATED_NSIS_DIR = REPO_ROOT / "src-tauri" / "target" / "release" / "nsis"

# NSIS resolves these itself (no translation needed from us).
NSIS_BUILTINS = {"^Name", "^NameDA", "^Version", "^Caption", "^SetupCaption"}


def collect_hebrew_translations() -> tuple[set[str], set[str]]:
    """Return (installer_defined, uninstaller_defined) string-name sets."""
    if not HEB_SRC.exists():
        print(f"ERROR: {HEB_SRC} not found", file=sys.stderr)
        sys.exit(2)

    text = HEB_SRC.read_text(encoding="utf-8-sig")  # tolerate BOM

    # `!insertmacro TauriLangString NAME "text"` defines BOTH tables.
    macro_names = set(re.findall(r"!insertmacro\s+TauriLangString\s+(\w+)", text))

    # Direct `LangString NAME ${LANG_HEBREW} "..."` (installer only).
    # Negative lookbehind avoids matching `UninstallLangString` here.
    direct_lang = set(
        re.findall(r"(?<!Uninstall)LangString\s+(\w+)\s+\$\{LANG_HEBREW\}", text)
    )

    # Direct `UninstallLangString NAME ${LANG_HEBREW} "..."` (uninstaller only).
    direct_unlang = set(
        re.findall(r"UninstallLangString\s+(\w+)\s+\$\{LANG_HEBREW\}", text)
    )

    return (direct_lang | macro_names, direct_unlang | macro_names)


def collect_references() -> tuple[set[str], set[str]] | tuple[None, None]:
    """Walk the generated installer.nsi and bucket every `$(name)` by scope."""
    candidates = list(GENERATED_NSIS_DIR.rglob("installer.nsi"))
    if not candidates:
        return (None, None)

    nsi_text = candidates[0].read_text(encoding="utf-8", errors="replace")

    installer_refs: set[str] = set()
    uninstaller_refs: set[str] = set()
    scope_is_uninstall = False

    for line in nsi_text.splitlines():
        stripped = line.strip()

        # Track Function/Section scope. `un.*` prefix means uninstaller.
        m = re.match(r"^(Function|Section)\s+(?:/\S+\s+)?([^;\s]+)", stripped)
        if m:
            scope_is_uninstall = m.group(2).startswith(("un.", "un_"))

        if stripped in {"FunctionEnd", "SectionEnd"} or stripped.startswith(
            ("FunctionEnd", "SectionEnd")
        ):
            scope_is_uninstall = False

        for ref in re.findall(r"\$\(([a-zA-Z_][a-zA-Z0-9_.]*)\)", line):
            if ref in NSIS_BUILTINS or ref.startswith("^"):
                continue
            (uninstaller_refs if scope_is_uninstall else installer_refs).add(ref)

    return (installer_refs, uninstaller_refs)


def main() -> int:
    installer_defined, uninstaller_defined = collect_hebrew_translations()
    installer_refs, uninstaller_refs = collect_references()

    if installer_refs is None:
        print(
            "note: no generated installer.nsi found — run `npm run tauri build` "
            "first for the cross-check against build output. Source-file "
            "translation counts: "
            f"{len(installer_defined)} installer / "
            f"{len(uninstaller_defined)} uninstaller."
        )
        return 0

    missing_installer = sorted(installer_refs - installer_defined)
    missing_uninstaller = sorted(uninstaller_refs - uninstaller_defined)

    if missing_installer or missing_uninstaller:
        for name in missing_installer:
            print(
                f"ERROR [installer]: $({name}) is referenced but no "
                f"LangString ${{LANG_HEBREW}} exists in Hebrew.nsh",
                file=sys.stderr,
            )
        for name in missing_uninstaller:
            print(
                f"ERROR [uninstaller]: $({name}) is referenced inside an "
                f"un.* block but no UninstallLangString ${{LANG_HEBREW}} "
                f"exists in Hebrew.nsh",
                file=sys.stderr,
            )
        total = len(missing_installer) + len(missing_uninstaller)
        print(
            f"\n{total} missing NSIS Hebrew translation(s). Fix Hebrew.nsh "
            "(prefer the `TauriLangString` macro that populates both tables).",
            file=sys.stderr,
        )
        return 1

    print(
        f"OK: Hebrew.nsh covers all {len(installer_refs)} installer + "
        f"{len(uninstaller_refs)} uninstaller references in the built "
        "installer.nsi."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
