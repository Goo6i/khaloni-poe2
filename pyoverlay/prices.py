"""Price data: poe.ninja currency exchange + poe2scout uniques.

Ported from the Rust core (ninja.rs / scout.rs), including the 18 verified
exchange types and the divine-denominated contract. Fetches are cached to
disk with a stale fallback so a network blip never zeroes the table.
"""
from __future__ import annotations

import json
import time
import urllib.parse
import urllib.request
from pathlib import Path

UA = "poe2-lens/0.2 (personal overlay)"
NINJA = "https://poe.ninja/poe2/api/economy/exchange/current/overview"
SCOUT = "https://api.poe2scout.com"

# Every exchange type that trades in the in-game currency exchange
# (live-verified 2026-07-23). Empty ones this league are skipped harmlessly.
EXCHANGE_TYPES = [
    "Currency", "Fragments", "Essences", "Runes", "UncutGems",
    "LineageSupportGems", "Omens", "Catalysts", "Artifacts", "SoulCores",
    "Talismans", "Expedition", "Ritual", "Breach", "Delirium", "Abyss",
    "Idols", "Verisium",
]


def _get(url: str, timeout: float = 12.0) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read().decode("utf-8")


class Prices:
    """name -> exalted price, for currency-exchange items and uniques."""

    def __init__(self, league: str, cache_dir: str | Path):
        self.league = league
        self.cache = Path(cache_dir)
        self.cache.mkdir(parents=True, exist_ok=True)
        self.exalted: dict[str, float] = {}   # name -> exalted value
        self.divine_rate: float | None = None  # exalted per divine
        self.uniques: dict[str, float] = {}
        self.stale = False

    # --- currency exchange ---------------------------------------------
    def _fetch_exchange(self) -> dict[str, float]:
        out: dict[str, float] = {}
        for typ in EXCHANGE_TYPES:
            url = f"{NINJA}?league={urllib.parse.quote(self.league)}&type={typ}"
            try:
                d = json.loads(_get(url))
            except Exception:
                continue
            core = d.get("core", {})
            rate = core.get("rates", {}).get("exalted")
            if core.get("primary") != "divine" or not rate or rate <= 0:
                continue
            if typ == "Currency":
                # Divine Orb's own exalted rate is the div<->ex reference.
                self.divine_rate = float(rate)
            for item, line in zip(d.get("items", []), d.get("lines", [])):
                pv = line.get("primaryValue")
                name = item.get("name")
                if name and pv:
                    out[name] = float(pv) * float(rate)
        return out

    # --- poe2scout uniques ---------------------------------------------
    def _fetch_uniques(self) -> dict[str, float]:
        try:
            filters = json.loads(_get(f"{SCOUT}/Realms/poe2/Filters"))
        except Exception:
            return {}
        cats = []
        for f in filters.get("Filters", []):
            if f.get("ItemKind") == "unique" and f.get("Category") not in cats:
                cats.append(f.get("Category"))
        out: dict[str, float] = {}
        for cat in cats:
            page = 1
            while True:
                url = (f"{SCOUT}/poe2/Leagues/{urllib.parse.quote(self.league)}"
                       f"/Uniques/ByCategory?category={cat}&page={page}&perPage=250")
                try:
                    d = json.loads(_get(url))
                except Exception:
                    break
                for it in d.get("Items", []):
                    price = it.get("CurrentPrice")
                    name = it.get("Name")
                    if name and price and price > 0:
                        out[name] = float(price)
                if page >= d.get("Pages", 1):
                    break
                page += 1
        return out

    # --- public --------------------------------------------------------
    def refresh(self) -> None:
        ex = self._fetch_exchange()
        uq = self._fetch_uniques()
        if ex:
            self.exalted = ex
            self.stale = False
            self._save("exchange.json", ex)
            if self.divine_rate:
                self._save("divine_rate.json", {"rate": self.divine_rate})
        else:
            ex_c = self._load("exchange.json")
            if ex_c:
                self.exalted = ex_c
                self.stale = True
            dr = self._load("divine_rate.json")
            if dr:
                self.divine_rate = dr.get("rate")
        if uq:
            self.uniques = uq
            self._save("uniques.json", uq)
        else:
            self.uniques = self._load("uniques.json") or {}

    def _save(self, fn: str, obj) -> None:
        (self.cache / fn).write_text(json.dumps(obj))

    def _load(self, fn: str):
        p = self.cache / fn
        if p.exists():
            try:
                return json.loads(p.read_text())
            except Exception:
                return None
        return None

    def lookup(self, name: str) -> float | None:
        return self.exalted.get(name) or self.uniques.get(name)


if __name__ == "__main__":
    import sys
    league = sys.argv[1] if len(sys.argv) > 1 else "Runes of Aldur"
    p = Prices(league, "/tmp/pyoverlay-cache")
    t = time.time()
    p.refresh()
    print(f"league={league}  exchange={len(p.exalted)} items  "
          f"uniques={len(p.uniques)}  divine_rate={p.divine_rate}  "
          f"stale={p.stale}  ({time.time()-t:.1f}s)")
    for probe in ["Divine Orb", "Exalted Orb", "Uruk's Smelting"]:
        print(f"  {probe!r}: {p.lookup(probe)} ex")
