"""Run the release audit with a scoped exception for RMCP's Chrono dependency."""

import subprocess
import sys
import tomllib
from pathlib import Path


def chrono_only_from_rmcp(packages):
    consumers = {
        package["name"]
        for package in packages
        if any(dep.split()[0] == "chrono" for dep in package.get("dependencies", []))
    }
    if consumers == {"rmcp"}:
        return True
    if consumers != {"rmcp", "schemars"}:
        return False

    # Skills.rs also uses Schemars. Check who enables its Chrono feature,
    # not who depends on Schemars itself. Pruning RMCP exposes any other enabler.
    tree = subprocess.check_output(
        [
            "cargo", "tree", "--locked", "--all-features", "--target", "all",
            "--edges", "features", "--invert", "schemars", "--prune", "rmcp",
            "--prefix", "depth", "--depth", "2", "--no-dedupe",
        ],
        text=True,
    ).splitlines()
    for line, following in zip(tree, tree[1:] + [""]):
        if line == '1schemars feature "chrono04"':
            return not following.startswith("2")
    return False


def main():
    with Path("Cargo.lock").open("rb") as lockfile:
        packages = tomllib.load(lockfile)["package"]
    command = ["cargo", "audit", "--deny", "warnings"]
    if chrono_only_from_rmcp(packages):
        print("Allowing RUSTSEC-2026-0543 for Chrono required by RMCP only.", file=sys.stderr)
        command.extend(["--ignore", "RUSTSEC-2026-0543"])
    return subprocess.call(command)


if __name__ == "__main__":
    sys.exit(main())
