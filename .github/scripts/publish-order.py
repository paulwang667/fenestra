#!/usr/bin/env python3
"""Print the workspace's publishable crates in dependency order, one per line.

The release workflow used to carry this list by hand, and it drifted twice: a
new crate (fenestra-a2ui) never got added even though a published crate
depends on it, and fenestra-markdown sat *after* the crate that reaches it
through a2ui. Either mistake fails a tag halfway through, once some crates are
already on crates.io and cannot be unpublished. So derive the order instead of
remembering it.

Dev-dependencies count as edges. `cargo publish` verifies a package by
building it, and that build resolves dev-dependencies from the registry -- so
fenestra-kit, whose golden tests dev-depend on fenestra-shell, cannot publish
until shell is up. Build-dependencies count for the same reason.

Crates marked `publish = false` are skipped, along with any edge pointing at
them.

Publishing also stops if any in-workspace dependency asks for a version its
target no longer has. That is not a hypothetical: the facade's dev-dependency
on fenestra-looks restated `version = "0.40.0"` by hand, so a workspace bump
to 0.41.0 left the requirement unsatisfiable by the local crate — and cargo
answers that by quietly resolving the *published* 0.40.0 instead, building
examples against a copy of the crate that is not in the checkout.
"""

from __future__ import annotations

import json
import subprocess
import sys


def workspace_metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(out.stdout)


def check_internal_versions(metadata: dict) -> None:
    """Every in-workspace dependency must name its target's current version.

    Cargo normalizes `version = "0.41.0"` to the requirement `^0.41.0`, and
    the convention here is that internal crates are always pinned to the
    exact version in the tree — so anything else is a stale hand-copied
    number, not a deliberate range.
    """
    versions = {pkg["name"]: pkg["version"] for pkg in metadata["packages"]}
    stale = [
        f"{pkg['name']} wants {dep['name']} {dep['req']}, "
        f"but it is {versions[dep['name']]}"
        for pkg in metadata["packages"]
        for dep in pkg["dependencies"]
        if dep["name"] in versions and dep["req"] != f"^{versions[dep['name']]}"
    ]
    if stale:
        raise SystemExit(
            "internal version requirements are out of date:\n  " + "\n  ".join(stale)
        )


def publish_order(metadata: dict) -> list[str]:
    # `publish` is null when unrestricted, or a (possibly empty) registry list.
    packages = {
        pkg["name"]: pkg
        for pkg in metadata["packages"]
        if pkg.get("publish") is None or pkg["publish"]
    }

    # Only edges *inside* the workspace constrain the order; everything else is
    # already on crates.io.
    needs = {
        name: {
            dep["name"]
            for dep in pkg["dependencies"]
            if dep["name"] in packages and dep["name"] != name
        }
        for name, pkg in packages.items()
    }

    ordered: list[str] = []
    done: set[str] = set()
    while len(ordered) < len(needs):
        ready = sorted(n for n in needs if n not in done and needs[n] <= done)
        if not ready:
            stuck = sorted(n for n in needs if n not in done)
            raise SystemExit(
                "publish order is cyclic; cargo cannot publish these in any "
                f"order: {', '.join(stuck)}"
            )
        ordered.extend(ready)
        done.update(ready)
    return ordered


def main() -> None:
    metadata = workspace_metadata()
    check_internal_versions(metadata)
    for name in publish_order(metadata):
        print(name)


if __name__ == "__main__":
    main()
