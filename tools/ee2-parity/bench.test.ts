import fs from "fs";
import path from "path";
import { parseClipboard } from "@/parser";
import { beforeAll, describe, it } from "vitest";
import { setupTests } from "@specs/vitest.setup";
import { init } from "@/assets/data";
import { createPresets } from "@/web/price-check/filters/create-presets";
import { createTradeRequest } from "@/web/price-check/trade/pathofexile-trade";
import * as items from "./Parser/items";

const IN = process.env.BENCH_IN!;
const OUT = process.env.BENCH_OUT!;

describe("bench", () => {
  beforeAll(async () => {
    setupTests();
    await init("en");
  });
  it("dumps", () => {
    if (process.env.BENCH_EXPORT) {
      for (const [k, v] of Object.entries(items)) {
        const raw = (v as { rawText?: string })?.rawText;
        if (raw) fs.writeFileSync(path.join(IN, `ee2-${k}.txt`), raw);
      }
    }
    for (const f of fs.readdirSync(IN).filter((f) => f.endsWith(".txt"))) {
      const text = fs.readFileSync(path.join(IN, f), "utf8");
      const parsed = parseClipboard(text);
      if (!parsed.isOk()) {
        fs.writeFileSync(path.join(OUT, f + ".err"), String(parsed._unsafeUnwrapErr()));
        continue;
      }
      const item = parsed._unsafeUnwrap();
      try {
        const { presets, active } = createPresets(item, {
          league: "Forbidden Rites",
          currency: undefined,
          listingType: "securable",
          collapseListings: "api",
          activateStockFilter: false,
          searchStatRange: 10,
          useEn: true,
          defaultAllSelected: !!process.env.BENCH_ALL,
        });
        const p = presets.find((p) => p.id === active)!;
        const body = createTradeRequest(p.filters, p.stats, item);
        const ui = p.stats.map((s) => ({
          text: s.text, tag: s.tag, ids: s.tradeId, disabled: s.disabled,
          hidden: s.hidden, min: s.roll?.min, max: s.roll?.max, value: s.roll?.value,
        }));
        fs.writeFileSync(path.join(OUT, f + ".json"), JSON.stringify({ preset: active, body, ui }, null, 1));
      } catch (e) {
        fs.writeFileSync(path.join(OUT, f + ".err"), String((e as Error).stack));
      }
    }
  });
});
