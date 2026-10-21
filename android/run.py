"""Install the APK on the connected device, run it, and collect what happened.

    python android/run.py                # install, start at the menus, watch 120 s
    python android/run.py --match        # straight into Killhouse TDM with bots, timed (COD4RW_PERF)
    python android/run.py --match 300    # ... for 300 s
    python android/run.py --no-install   # run what's installed
    python android/run.py --match --env TEXTURE_MAX_SIZE=512   # extra env.txt lines
    python android/run.py --match --map mp_crossfire --bots 3  # another map, bots a team

Output in target/android/runs/<time>/: logcat.txt, cod4rw.log, end.png, and
the fps lines and the peak memory (PSS, sampled every 10 s) printed at the end. The device must be unlocked; the script
waits for that (up to 5 minutes); the game keeps the screen on itself.
"""

import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SDK = os.environ.get("ANDROID_HOME", r"D:\android-sdk")
ADB = os.path.join(SDK, "platform-tools", "adb.exe")
PACKAGE = "com.cod4rewrite.game"
FILES = f"/sdcard/Android/data/{PACKAGE}/files"
APK = os.path.join(HERE, "out", "cod4rw.apk")


def adb(*args, check=True, capture=True):
    r = subprocess.run([ADB, *args], capture_output=capture, text=True, encoding="utf-8", errors="replace")
    if check and r.returncode != 0:
        sys.exit(f"adb {' '.join(args)} failed: {r.stderr.strip()}")
    return r.stdout if capture else ""


def shell(cmd, check=False):
    return adb("shell", cmd, check=check)


def locked():
    out = shell("dumpsys window")
    return "mDreamingLockscreen=true" in out or "isKeyguardShowing=true" in out or "mShowingLockscreen=true" in out


def main():
    args = sys.argv[1:]
    value = lambda flag: [args[i + 1] for i, a in enumerate(args[:-1]) if a == flag]
    match = "--match" in args
    env = value("--env")
    after_match = args[args.index("--match") + 1] if match and args.index("--match") + 1 < len(args) else ""
    seconds = int(after_match) if after_match.isdigit() else (180 if match else 120)
    game_args = [f"--map {m}" for m in value("--map")] + [f"--bots {b}" for b in value("--bots")]
    if game_args:
        env.append("GAME_ARGS=" + " ".join(game_args))

    devices = [l for l in adb("devices").splitlines()[1:] if l.strip()]
    if not any(l.endswith("\tdevice") for l in devices):
        sys.exit(f"no device ready: {devices or 'none'} (USB debugging on, and allowed on the device?)")

    if "--no-install" not in args:
        print(f"installing {APK} ...", flush=True)
        adb("install", "-r", APK)
    # Read CoD4 from shared storage (Download/Call of Duty 4).
    shell(f"appops set {PACKAGE} MANAGE_EXTERNAL_STORAGE allow")

    # A timed match (debug aids start one directly), or the menus.
    shell(f"mkdir -p {FILES}")
    lines = (["COD4RW_PERF=1"] if match else []) + env
    shell(f"rm -f {FILES}/env.txt")
    for line in lines:
        shell(f"echo '{line}' >> {FILES}/env.txt")

    shell("input keyevent KEYCODE_WAKEUP")
    waited = 0
    while locked():
        if waited == 0:
            print("the device is locked: unlock it (press A) ...", flush=True)
        if waited >= 300:
            sys.exit("still locked after 5 minutes")
        time.sleep(5)
        waited += 5

    run = os.path.join(HERE, "..", "target", "android", "runs", time.strftime("%Y%m%d-%H%M%S"))
    os.makedirs(run, exist_ok=True)
    shell(f"am force-stop {PACKAGE}")
    adb("logcat", "-c")
    shell(f"am start -n {PACKAGE}/android.app.NativeActivity", check=True)
    print(f"running {seconds} s ({'match' if match else 'menus'}) ...", flush=True)
    start = time.time()
    peak_mb = 0
    while time.time() - start < seconds:
        time.sleep(10)
        if not shell(f"pidof {PACKAGE}").strip():
            print("the game stopped", flush=True)
            break
        info = shell(f"dumpsys meminfo {PACKAGE}")
        m = re.search(r"TOTAL PSS:\s*(\d+)", info) or re.search(r"^\s*TOTAL\s+(\d+)", info, re.M)
        if m:
            peak_mb = max(peak_mb, int(m.group(1)) // 1024)

    with open(os.path.join(run, "end.png"), "wb") as f:
        f.write(subprocess.run([ADB, "exec-out", "screencap", "-p"], capture_output=True).stdout)
    log = adb("logcat", "-d")
    with open(os.path.join(run, "logcat.txt"), "w", encoding="utf-8") as f:
        f.write(log)
    adb("pull", f"{FILES}/cod4rw.log", os.path.join(run, "cod4rw.log"), check=False)
    alive = bool(shell(f"pidof {PACKAGE}").strip())
    shell(f"am force-stop {PACKAGE}")
    shell(f"rm -f {FILES}/env.txt")

    clean = re.compile(r"\x1b\[[0-9;]*m")
    print(f"\n{os.path.abspath(run)}")
    for line in log.splitlines():
        line = clean.sub("", line)
        if any(k in line for k in ("android: fps", "perf:", "map ready", "ui: ", "F libc", "panic", "CRASH", "ANR in", "FATAL")):
            print(line[line.find(" ", 30):].strip()[:220])
    print(f"peak memory (PSS): {peak_mb} MB")
    print("still running at the end" if alive else "NOT running at the end")


if __name__ == "__main__":
    main()
