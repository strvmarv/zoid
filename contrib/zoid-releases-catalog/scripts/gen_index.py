#!/usr/bin/env python3
"""Regenerate plugins/index.json from plugins/*.toml. Stdlib only (tomllib)."""
import json, sys, tomllib
from pathlib import Path

def build_index(plugins_dir: Path) -> dict:
    entries = []
    for toml_path in sorted(plugins_dir.glob("*.toml")):
        with toml_path.open("rb") as fh:
            data = tomllib.load(fh)
        p, s = data["plugin"], data["source"]
        entry = {
            "id": p["id"], "name": p.get("name", p["id"]),
            "kind": p["kind"], "description": p.get("description", ""),
            "source": {"repo": s["repo"], "ref": s["ref"]},
        }
        if "license" in p:
            entry["license"] = p["license"]
        entries.append(entry)
    entries.sort(key=lambda e: e["id"])
    return {"schema": 1, "plugins": entries}

def main(argv):
    plugins_dir = Path(argv[1]) if len(argv) > 1 else Path(__file__).resolve().parent.parent / "plugins"
    index = build_index(plugins_dir)
    out = json.dumps(index, indent=2, ensure_ascii=False) + "\n"
    (plugins_dir / "index.json").write_text(out, encoding="utf-8")
    print(f"wrote {plugins_dir/'index.json'} ({len(index['plugins'])} plugins)")

if __name__ == "__main__":
    main(sys.argv)
