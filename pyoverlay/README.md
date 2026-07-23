# pyoverlay — cross-platform Python rebuild (Phase 1)

Screen-reading PoE2 overlay: reward-panel pricing + expedition rumours.
Cross-platform (Windows-first); replaces the Linux-only Rust tool over
time. See docs/notes/specs/2026-07-24-cross-platform-pivot-design.md.

Phase 1 (done, verified): rumour recognizer (8/10 recall, 0 false
positives on 5 real fixtures), price data (poe.ninja exchange + poe2scout
uniques), spectacle capture, e2e console runner.

Run: `python3 -m pyoverlay.main --loop 2`   (from repo root)
Test: `python3 -m pyoverlay.test_rumours`
