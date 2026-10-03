# EE2 parity check

The price-check search (`core/src/ee2`, serialized by `Query::to_body`) is a
port of Exiled Exchange 2's. This directory holds it to the request EE2
builds for the same clipboard text, using EE2's own parser and request
builder run headless.

- `items/*.txt`: the corpus, 80 clipboard texts: EE2's own samples
  (`ee2-*`), one in-game bow (`ours-bow`), hand-written gear and non-gear
  kinds (`new-*`), and edge cases (`edge-*`: option stats, inverted ids, an
  anointment, an unrevealed desecrated mod, a trial key, an unknown line).
- `golden/*.json`: per item, the preset EE2 chose, its default body, and
  its body with every shown row switched on (`defaultAllSelected`).
- `data/`: the files that run read, from EE2 commit
  `cca30662bf31eaf38bd711e2ec1a6b899a06c40e`, the commit
  `refdata::EE2_COMMIT` pins: `stats.ndjson` and `items.ndjson`
  (`renderer/public/data/en`), `trade-stats.json` and `trade-items.json`
  (`renderer/specs/data`, EE2's snapshot of the trade site's catalogs).

`cargo test -p khaloni-poe2-core --test ee2_parity` compares against the
golden files offline. Regenerate them after an EE2 update, or after adding
items:

```sh
W=$(mktemp -d); mkdir -p $W/ee2out $W/ourout $W/all/ee2out $W/all/ourout
git clone --depth 1 https://github.com/Kvan7/Exiled-Exchange-2.git $W/ee2
cp tools/ee2-parity/bench.test.ts $W/ee2/renderer/specs/
(cd $W/ee2/renderer && npm ci --ignore-scripts && node src/assets/make-index-files.mjs)  # the stat index is a build artefact
E=$W/ee2/renderer; I=$PWD/tools/ee2-parity/items

(cd $E && BENCH_IN=$I BENCH_OUT=$W/ee2out npx vitest run specs/bench.test.ts)
(cd $E && BENCH_ALL=1 BENCH_IN=$I BENCH_OUT=$W/all/ee2out npx vitest run specs/bench.test.ts)
ls $W/ee2out/*.err 2>/dev/null   # EE2 could not build these: fix the text

DATA="$E/public/data/en/stats.ndjson $E/public/data/en/items.ndjson $E/specs/data/stats.json $E/specs/data/items.json"
cargo run -q -p khaloni-poe2-core --example bench_query -- $DATA $I $W/ourout
cargo run -q -p khaloni-poe2-core --example bench_query -- $DATA $I $W/all/ourout all
python3 tools/ee2-parity/compare.py $W        # default selection
python3 tools/ee2-parity/compare.py $W/all    # every shown row on

# once both print "different 0":
D=tools/ee2-parity/data
cp $E/public/data/en/stats.ndjson $E/public/data/en/items.ndjson $D/
cp $E/specs/data/stats.json $D/trade-stats.json; cp $E/specs/data/items.json $D/trade-items.json
python3 tools/ee2-parity/make-golden.py $W/ee2out $W/all/ee2out tools/ee2-parity/golden
```

`compare.py` ignores the order of stat groups and of filters inside a group,
which the trade API does not read; every id, bound, `disabled` flag and item
filter must match. It exits 0 only when every item is identical.
`BENCH_EXPORT=1` on the vitest run writes EE2's own sample items into the
items directory.

The best corpus is real items. With `KHALONI_ITEM_DUMP=<dir>` set, the app
writes the clipboard text of every item it price-checks into `<dir>`; drop
those files into `items/` and regenerate.

After an EE2 update, bump `refdata::EE2_COMMIT` to the commit the golden
files came from, so the app downloads the data the test passed on.
