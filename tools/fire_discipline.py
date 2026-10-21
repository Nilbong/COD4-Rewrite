"""Fire discipline from recordings: shots past hip range with sights under
80% up, shots with nobody in sight, time spent looking steeply down out of
fights: python tools/fire_discipline.py <dir|csv>..."""
import csv, glob, os, sys

for arg in sys.argv[1:]:
    files = sorted(glob.glob(os.path.join(arg, "**", "*.csv"), recursive=True)) if os.path.isdir(arg) else [arg]
    shots = hipfar = blind = rows_n = down = calm = 0
    for f in files:
        if f.endswith("-personas.csv"):
            continue
        rows = list(csv.DictReader(open(f, newline="")))[:-1]
        if not rows or "ads_level" not in rows[0]:
            continue
        for r in rows:
            if r["dead"] == "1":
                continue
            rows_n += 1
            n = int(r["shots"] or 0)
            if n:
                shots += n
                if r["edist"] and float(r["edist"]) > 350 and float(r["ads_level"] or 0) < 0.8 and "winchester" not in r["weapon"] and "m1014" not in r["weapon"]:
                    hipfar += n
                if not r["vis"]:
                    blind += n
            if not r["enemy"] and r["fire"] == "0":
                calm += 1
                if float(r["pitch"]) < -30:
                    down += 1
    print(f"{arg}: {shots} shots; past 350u with sights <80% up {hipfar / max(shots, 1):.0%}; with nobody in sight {blind / max(shots, 1):.0%}; "
          f"looking >30 deg down out of fights {down / max(calm, 1):.1%}")
