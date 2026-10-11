#!/usr/bin/env bash
# In-game proof run for the phone call-in: the streak test (UAV, then the
# helicopter: both the phone) with screenshots of the hands named by part
# and time (COD4RW_MOCAPSHOT). Silent (COD4RW_* runs have no sound device),
# 1280x720, time-limited; the exe copy is killed and deleted afterwards.
#
# run_ingame.sh <out dir> <hands.phone.mocap: 0 or 1> [seconds]
set -u
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$1"
knob="$2"
limit="${3:-150}"
work="$root/target/mocaptest_$knob"
exe="cod4rw_mocaptest_$knob.exe"
rm -rf "$work" "$out"
mkdir -p "$work" "$out"
cp "$root/target/fast/cod4rw.exe" "$work/$exe"
# Its own tuning file (the user's tuning.txt stays as it is).
grep -v '^hands\.phone\.mocap' "$root/tuning.txt" > "$work/tuning.txt"
echo "hands.phone.mocap = $knob   # default 1" >> "$work/tuning.txt"
cd "$root"
COD4RW_STREAKTEST="$out/streak" COD4RW_MOCAPSHOT="$out/shots" COD4RW_RES=1280x720 COD4RW_TUNING="$work/tuning.txt" \
    timeout "$limit" "$work/$exe" > "$out/log.txt" 2>&1
echo "exit $?"
taskkill //IM "$exe" //F > /dev/null 2>&1
sleep 1
rm -rf "$work"
ls "$out/shots" 2>/dev/null | wc -l
