"""Build the Android APK: android/out/cod4rw.apk (see README.md).

    python android/build.py            # cargo build --release, then package
    python android/build.py --package  # package the last build only

Needs the SDK/NDK in D:\\android-sdk (ANDROID_HOME overrides) and a JDK's
java/keytool on PATH or in C:\\Program Files\\Java.
"""

import glob
import os
import shutil
import subprocess
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
SDK = os.environ.get("ANDROID_HOME", r"D:\android-sdk")
TOOLS = os.path.join(SDK, "build-tools", "34.0.0")
ANDROID_JAR = os.path.join(SDK, "platforms", "android-34", "android.jar")
NDK_BIN = os.path.join(SDK, "ndk", "android-ndk-r27d", "toolchains", "llvm", "prebuilt", "windows-x86_64", "bin")
LIB = os.path.join(HERE, "..", "target", "android", "aarch64-linux-android", "release", "libcod4rw.so")
OUT = os.path.join(HERE, "out")
WORK = os.path.join(HERE, "..", "target", "android", "apk")
KEYSTORE = os.path.join(HERE, "debug.keystore")


def java_tool(name):
    found = shutil.which(name)
    if found:
        return found
    for path in glob.glob(rf"C:\Program Files\Java\*\bin\{name}.exe"):
        return path
    sys.exit(f"{name} not found (install a JDK)")


def run(*cmd, **kw):
    print(">", " ".join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], check=True, **kw)


def dependencies(path):
    """A manifest's [dependencies] as {name: line} (simple TOML only)."""
    import re
    deps, inside = {}, False
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if line.startswith("["):
            inside = line == "[dependencies]"
        elif inside and (m := re.match(r"([A-Za-z0-9_-]+)\s*[.=]", line)):
            deps[m.group(1)] = line
    return deps


def check_dependencies():
    """Cargo.toml here repeats crates/game's dependencies: say when they drift."""
    import re
    game = dependencies(os.path.join(HERE, "..", "crates", "game", "Cargo.toml"))
    ours = dependencies(os.path.join(HERE, "Cargo.toml"))
    for name in sorted(set(game) - set(ours)):
        print(f"WARNING: crates/game depends on {name}, android/Cargo.toml doesn't")
    features = lambda line: set(re.findall(r'"([^"]+)"', line.split("features", 1)[1])) if "features" in line else set()
    missing = features(game.get("bevy", "")) - features(ours.get("bevy", ""))
    if missing:
        print(f"WARNING: bevy features in crates/game missing here: {sorted(missing)}")


def main():
    check_dependencies()
    if "--package" not in sys.argv:
        # -j 4: other builds share this machine.
        run("cargo", "build", "--release", "-j", "4", cwd=HERE)
    os.makedirs(WORK, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)

    # Resources (the icon) and the manifest.
    res_zip = os.path.join(WORK, "res.zip")
    run(os.path.join(TOOLS, "aapt2.exe"), "compile", "--dir", os.path.join(HERE, "res"), "-o", res_zip)
    base = os.path.join(WORK, "base.apk")
    run(os.path.join(TOOLS, "aapt2.exe"), "link", "-o", base, "-I", ANDROID_JAR,
        "--manifest", os.path.join(HERE, "AndroidManifest.xml"), res_zip)

    # The game, stored uncompressed so Android maps it straight from the APK
    # (extractNativeLibs=false).
    stripped = os.path.join(WORK, "libcod4rw.so")
    run(os.path.join(NDK_BIN, "llvm-strip.exe"), "--strip-all", "-o", stripped, LIB)
    unaligned = os.path.join(WORK, "unaligned.apk")
    shutil.copyfile(base, unaligned)
    with zipfile.ZipFile(unaligned, "a") as z:
        z.write(stripped, "lib/arm64-v8a/libcod4rw.so", compress_type=zipfile.ZIP_STORED)
    aligned = os.path.join(WORK, "aligned.apk")
    run(os.path.join(TOOLS, "zipalign.exe"), "-f", "-p", "4", unaligned, aligned)

    # Signed with a local debug key (made once).
    if not os.path.exists(KEYSTORE):
        run(java_tool("keytool"), "-genkeypair", "-keystore", KEYSTORE, "-storepass", "android", "-keypass", "android",
            "-alias", "androiddebugkey", "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000",
            "-dname", "CN=Android Debug,O=Android,C=US")
    apk = os.path.join(OUT, "cod4rw.apk")
    run(java_tool("java"), "-jar", os.path.join(TOOLS, "lib", "apksigner.jar"), "sign", "--ks", KEYSTORE,
        "--ks-pass", "pass:android", "--key-pass", "pass:android", "--out", apk, aligned)
    print(f"APK: {os.path.abspath(apk)} ({os.path.getsize(apk) / 1e6:.0f} MB)")


if __name__ == "__main__":
    main()
