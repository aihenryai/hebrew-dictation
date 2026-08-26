"""Rewrite latest.json's API asset URLs to public download URLs.

tauri-action always records API asset URLs
(https://api.github.com/repos/OWNER/REPO/releases/assets/<id>) in latest.json,
because those also resolve for private repositories. They do not work for the
updater on a public repo: without an `Accept: application/octet-stream` header
GitHub answers that URL with the asset's JSON *metadata*, and Tauri's updater
sends no such header — so it downloads JSON where it expects an installer and
the update fails. Confirmed by hand against a real release: the API URL
answers `content-type: application/json`, the browser_download_url answers
`application/octet-stream`.

tauri-action exposes no input to change this, hence this post-processing step.

Idempotent: only entries still pointing at the API are touched, so running
this once per matrix job leaves the other platform's already-rewritten entry
alone.

Usage: fix_updater_urls.py <latest.json> <assets.json>
  Rewrites <latest.json> in place. Exits 1 if an entry cannot be resolved,
  rather than uploading a file that would silently break auto-update.
"""

import io
import json
import sys

API_PREFIX = "https://api.github.com/"


def main(latest_path: str, assets_path: str) -> int:
    latest = json.load(io.open(latest_path, encoding="utf-8"))
    assets = json.load(io.open(assets_path, encoding="utf-8"))
    by_id = {a["id"]: a["browser_download_url"] for a in assets}

    changed = False
    for name, platform in latest.get("platforms", {}).items():
        url = platform.get("url", "")
        if not url.startswith(API_PREFIX):
            print(f"  {name}: already a public URL, left alone")
            continue

        try:
            asset_id = int(url.rstrip("/").split("/")[-1])
        except ValueError:
            print(f"ERROR: {name}: cannot parse an asset id out of {url}")
            return 1

        if asset_id not in by_id:
            # Better to fail the job than to upload a latest.json whose URL
            # 404s for every user on this platform.
            print(f"ERROR: {name}: asset {asset_id} is not on this release")
            return 1

        platform["url"] = by_id[asset_id]
        changed = True
        print(f"  {name}: rewrote -> {platform['url']}")

    if changed:
        io.open(latest_path, "w", encoding="utf-8").write(
            json.dumps(latest, ensure_ascii=False, indent=2) + "\n"
        )
        print("latest.json rewritten")
    else:
        print("nothing to rewrite")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    sys.exit(main(sys.argv[1], sys.argv[2]))
