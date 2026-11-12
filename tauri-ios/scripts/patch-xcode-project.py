#!/usr/bin/env python3
"""Adds the system libraries/frameworks the LinuxRunner core needs to Tauri's generated XcodeGen spec.

The Rust code is built as a static library, so `-lsqlite3` etc. requested by build.rs never reach the final
Xcode link; they have to be dependencies of the Xcode target instead.
usage: patch-xcode-project.py <path to gen/apple/project.yml>
"""
import re
import sys

path = sys.argv[1]
lines = open(path).read().split("\n")
wanted = ["libsqlite3.tbd", "libc++.tbd", "libresolv.tbd", "Foundation.framework", "CoreFoundation.framework"]

# Find an existing "- sdk: X" dependency line in the iOS target and add ours next to it.
idx = next((i for i, l in enumerate(lines) if re.match(r"^\s*-\s*sdk:\s*\S+", l)), None)
if idx is None:
    print("ERROR: no '- sdk:' dependency found in", path, file=sys.stderr)
    print("\n".join(lines), file=sys.stderr)
    sys.exit(1)

indent = re.match(r"^(\s*)-", lines[idx]).group(1)
present = {m.group(1) for l in lines for m in [re.match(r"^\s*-\s*sdk:\s*(\S+)", l)] if m}
new = [f"{indent}- sdk: {name}" for name in wanted if name not in present]
lines[idx + 1:idx + 1] = new
open(path, "w").write("\n".join(lines))
print("Added:", ", ".join(w for w in wanted if w not in present) or "(nothing, already present)")
