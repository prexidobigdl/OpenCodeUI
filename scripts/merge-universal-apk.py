#!/usr/bin/env python3
"""Merge per-ABI APKs into a single universal APK.

Usage:
    merge-universal-apk.py <arm64.apk> <armv7.apk> <out.apk>

The arm64 APK provides every entry except ``lib/armeabi-v7a/**``; the armv7
APK contributes only its ``lib/armeabi-v7a/**`` tree. Compression method and
external attributes are preserved per entry so that zipalign/apksigner can
still align the native libraries and re-sign the package afterwards.

Signature entries (``META-INF/**``) are dropped: the merged archive is signed
again from scratch by apksigner.
"""

import sys
import zipfile

SKIP_PREFIXES = ("META-INF/",)


def copy_entries(src: zipfile.ZipFile, out: zipfile.ZipFile, want):
    for info in src.infolist():
        name = info.filename
        if name.startswith(SKIP_PREFIXES):
            continue
        if not want(name):
            continue
        data = src.read(name)
        target = zipfile.ZipInfo(name, date_time=info.date_time)
        target.compress_type = info.compress_type
        target.external_attr = info.external_attr
        target.internal_attr = info.internal_attr
        target.create_system = info.create_system
        out.writestr(target, data)


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2

    arm64_path, armv7_path, out_path = sys.argv[1:4]

    with zipfile.ZipFile(arm64_path) as arm64, zipfile.ZipFile(armv7_path) as armv7:
        arm64_abis = {n.split("/")[1] for n in arm64.namelist() if n.startswith("lib/")}
        armv7_abis = {n.split("/")[1] for n in armv7.namelist() if n.startswith("lib/")}
        if not arm64_abis:
            raise SystemExit(f"no lib/ entries found in {arm64_path}")
        if not armv7_abis:
            raise SystemExit(f"no lib/ entries found in {armv7_path}")

        print(f"arm64 apk abis: {sorted(arm64_abis)}")
        print(f"armv7 apk abis: {sorted(armv7_abis)}")

        with zipfile.ZipFile(out_path, "w") as out:
            copy_entries(
                arm64,
                out,
                lambda name: not name.startswith("lib/armeabi-v7a/"),
            )
            copy_entries(
                armv7,
                out,
                lambda name: name.startswith("lib/armeabi-v7a/"),
            )

    with zipfile.ZipFile(out_path) as merged:
        abis = sorted({n.split("/")[1] for n in merged.namelist() if n.startswith("lib/")})
        print(f"merged apk abis: {abis}")
        if "arm64-v8a" not in abis or "armeabi-v7a" not in abis:
            raise SystemExit(f"unexpected abi set in merged apk: {abis}")

    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
