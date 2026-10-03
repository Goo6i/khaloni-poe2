"""Writes golden/<item>.json from two EE2 runs of bench.test.ts.

Usage: make-golden.py <default ee2out> <BENCH_ALL ee2out> <golden dir>
Each golden file holds the preset EE2 chose, the body of its default
selection, and the body with every shown row switched on.
"""
import glob, json, os, sys

default_dir, all_dir, out_dir = sys.argv[1:4]
for old in glob.glob(os.path.join(out_dir, "*.json")):
    os.remove(old)
for f in sorted(glob.glob(os.path.join(default_dir, "*.json"))):
    name = os.path.basename(f)
    default = json.load(open(f))
    everything = json.load(open(os.path.join(all_dir, name)))
    golden = {"preset": default["preset"], "body": default["body"], "body_all": everything["body"]}
    with open(os.path.join(out_dir, name.removesuffix(".txt.json") + ".json"), "w") as out:
        json.dump(golden, out, indent=1, sort_keys=True)
        out.write("\n")
