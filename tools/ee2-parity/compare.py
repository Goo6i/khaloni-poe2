"""Strict diff of the request body we build against EE2's, per item.

Usage: compare.py <workdir> [-v]   (workdir holds ee2out/ and ourout/)
Bodies must be equal as the trade API sees them: the order of stat groups
and of filters inside a group carries no meaning and is ignored; everything
else (ids, bounds, disabled flags, every item filter) must match.
"""
import glob, json, os, sys

S = sys.argv[1]
verbose = "-v" in sys.argv


def canon(body):
    q = json.loads(json.dumps(body["query"]))
    groups = []
    for g in q.pop("stats", []):
        fs = sorted(
            json.dumps({k: v for k, v in f.items() if v not in (None, {})} | {"disabled": bool(f.get("disabled"))}, sort_keys=True)
            for f in g.get("filters", [])
        )
        if not fs and g.get("type") == "and":
            continue
        groups.append(json.dumps({"type": g["type"], "value": g.get("value"), "disabled": bool(g.get("disabled")), "filters": fs}, sort_keys=True))
    q["stats"] = sorted(groups)
    if not q.get("filters"):
        q.pop("filters", None)
    return {"query": q, "sort": body.get("sort")}


def flat(d, prefix=""):
    out = {}
    for k, v in d.items():
        if isinstance(v, dict):
            out.update(flat(v, f"{prefix}{k}."))
        else:
            out[prefix + k] = v
    return out


same, diff = 0, []
for f in sorted(glob.glob(S + "/ee2out/*.json")):
    n = os.path.basename(f)
    e = json.load(open(f))
    try:
        o = json.load(open(S + "/ourout/" + n))
    except FileNotFoundError:
        diff.append(n); print("=====", n, "\n   no output from us"); continue
    if "body" not in o:
        diff.append(n); print("=====", n, "\n   OURS", o); continue
    ce, co = canon(e["body"]), canon(o["body"])
    if ce == co:
        same += 1
        continue
    diff.append(n)
    print("=====", n, f"({e['preset']})")
    fe, fo = flat({k: v for k, v in ce["query"].items() if k != "stats"}), flat({k: v for k, v in co["query"].items() if k != "stats"})
    for k in sorted(set(fe) | set(fo)):
        if fe.get(k) != fo.get(k):
            print(f"   {k}: EE2 {fe.get(k)!r} | OURS {fo.get(k)!r}")
    se, so = set(ce["query"]["stats"]), set(co["query"]["stats"])
    for g in sorted(se - so):
        print("   only EE2 :", g[:400] if not verbose else g)
    for g in sorted(so - se):
        print("   only OURS:", g[:400] if not verbose else g)
for f in sorted(glob.glob(S + "/ee2out/*.err")):
    print("EE2 could not build:", os.path.basename(f))
print(f"identical {same}, different {len(diff)}")
sys.exit(1 if diff else 0)
