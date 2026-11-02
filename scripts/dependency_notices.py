"""Collect original notices and exact crate source links for the container."""
import argparse
from pathlib import Path
import tomllib


def collect(registry, lockfile, output):
    packages = tomllib.loads(lockfile.read_text(encoding="utf-8"))["package"]
    sections = ["Cadence dependency notices\nOriginal crate sources: https://crates.io\n"]
    for package in packages:
        if not package.get("source", "").startswith("registry+"):
            continue
        name, version = package["name"], package["version"]
        roots = list(registry.glob(f"*/{name}-{version}"))
        if not roots:
            continue  # Dependencies not downloaded for this target are not linked.
        root = roots[0]
        manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        sections.append(f"\n{'=' * 72}\n{name} {version}\nLicense: {manifest.get('license', 'See original notices')}\nSource: https://crates.io/api/v1/crates/{name}/{version}/download\nAuthors: {', '.join(manifest.get('authors', []))}\n")
        candidates = set()
        for file in root.rglob("*"):
            if file.is_file() and file.name.lower().startswith(("license", "licence", "copying", "copyright", "notice")):
                candidates.add(file)
        license_file = manifest.get("license-file")
        if license_file and (root / license_file).is_file():
            candidates.add(root / license_file)
        for file in sorted(candidates):
            try:
                content = file.read_text(encoding="utf-8")
            except (UnicodeError, OSError):
                continue
            sections.append(f"\n--- {file.relative_to(root)} ---\n{content}\n")
        if not candidates:
            sections.append("The published crate does not include a separate notice file. Its license declaration and exact source link are recorded above.\n")
    output.write_text("\n".join(sections), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry", type=Path, required=True)
    parser.add_argument("--lockfile", type=Path, default=Path("Cargo.lock"))
    parser.add_argument("--output", type=Path, default=Path("DEPENDENCY_LICENSES.txt"))
    args = parser.parse_args()
    collect(args.registry, args.lockfile, args.output)
