"""Verify the isolated model consumer cannot acquire Xtext by feature unification."""
import json
from pathlib import Path
import subprocess
from check_text_separation import check_ownership


def main():
    root = Path(__file__).resolve().parent.parent
    errors = check_ownership(root)
    if errors:
        raise SystemExit("\n".join(errors))
    result = subprocess.run(
        ["cargo", "metadata", "--manifest-path", "tests/model-only/Cargo.toml",
         "--format-version", "1", "--locked"],
        cwd=root, check=True, text=True, capture_output=True,
    )
    metadata = json.loads(result.stdout)
    packages = {p["id"]: p["name"] for p in metadata["packages"]}
    forbidden = {"intersect-text", "intersect-xtext", "intersect-language", "intersect-web", "intersect-text-acceptance"}
    for node in metadata["resolve"]["nodes"]:
        name = packages[node["id"]]
        if name in forbidden:
            raise SystemExit(f"Forbidden model-only dependency: {name}")
        if name == "intersect-model" and "text-compiler" in node["features"]:
            raise SystemExit("Model-only consumer enabled the internal Xtext compiler")
    if "intersect-model" not in packages.values():
        raise SystemExit("Consumer did not exercise intersect-model")
    print("Model-only dependency graph excludes Xtext and the legacy language facade.")


if __name__ == "__main__":
    main()
