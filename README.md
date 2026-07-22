# poe2-lens

A price overlay for Path of Exile 2 on Linux. It reads the Runeshape
Combinations panel off the screen, looks every reward up on poe.ninja, and
draws the price next to each row while you play. Point at any item and press
F7 and it prices that too, including rare items, which it appraises against
live listings on the official trade site.

I built this because nothing else runs on my machine. The tools that do this
job are Windows programs built on the Windows OCR API. The one C++ tool with
a Linux build, RuneHelper, captures through X11 and logs "Wayland is not
supported yet" on a Wayland session, and my game runs inside gamescope on KDE
Wayland where an X11 capture sees nothing at all. So the choice was live
without prices or build the Linux version myself.

## What it does

- Prices every row of a Runeshape panel, in game, without alt-tabbing. Stack
  counts are respected: a `3x Exalted Orb` row shows the total and the each
  price.
- Skill gem rows price by their exact level against the uncut gem market.
  Rows it cannot price honestly show `?` instead of a guess. That rule is
  absolute: a wrong price never renders. When two near-identical items like
  Lesser and Greater Jeweller's Orb are too close to call from a noisy read,
  it refuses and shows `?` rather than picking one.
- Marks the highest-value option on pick-one panels with a gold border, so
  the choice takes zero reading.
- Follows the panel while you scroll. The scroll offset is measured from the
  image itself between frames, so labels move with their rows instead of
  waiting for the next text read.
- F7 prices whatever your cursor is on, anywhere: inventory, stash, vendor,
  even items linked in chat. Fixed-value items answer from the local
  price table with no network round trip. Rares run a filtered search against pathofexile.com/trade2 and
  the popup lists the cheapest matching listings with seller names.
- Prices refresh every 10 minutes on their own. If the network dies, the
  last good table stays up, labels are marked `(old)`, and it retries every
  minute until fresh data lands. There is no refresh key because you should
  never have to think about freshness.

## How it reads the screen

Screen capture goes through the desktop portal into PipeWire, which is the
only capture path that can see a Wayland-native game window. You grant the
permission once; a restore token skips the dialog on every later launch.

Finding the rows costs almost nothing: the reward rows are bright parchment
bars, and a per-row brightness profile finds them in about 2 milliseconds
without any text recognition. That same profile is what detects scrolling
and what tells the overlay the panel closed, which is why labels vanish in
two capture ticks instead of waiting on a slow text pass.

Reading the rows is a two-tier system. The first time a reward appears,
Tesseract reads the strip and the matched result is stored as a template,
pixels included, keyed to the item and stack count. Every later encounter of
that reward is identified by correlating against the stored strip, which
takes 0.67 milliseconds per row on my machine and cannot misread, because it
is matching the game's own rendering against itself. Tesseract stays around
only as the teacher for rewards the store has never seen. I measured the
correlation engine against 420 row images from a scrolling session before
trusting it: zero wrong identifications.

The overlay itself is a layer-shell surface with an empty input region, so
it draws above the fullscreen game and every click passes through to the
game. A small KWin script reports the game window's position over DBus, so
the labels stay anchored if the window moves.

## Setup

This currently targets Linux with KDE Plasma on Wayland. A Windows build is
planned; the code is split so the platform layer is the only part that
changes.

Packages, on Arch and derivatives:

```
sudo pacman -S --needed tesseract tesseract-data-eng leptonica slurp
```

The F7 price check types Ctrl+C into the game through a virtual keyboard,
which needs `/dev/uinput` access without root:

```
sudo usermod -aG input $USER
echo 'KERNEL=="uinput", GROUP="input", MODE="0660"' | sudo tee /etc/udev/rules.d/99-poe2lens-uinput.rules
sudo udevadm control --reload-rules && sudo udevadm trigger /dev/uinput
```

Log out and back in once after that; group membership only applies to new
sessions.

Build and set up:

```
cargo build --release -p poe2-lens
./target/release/poe2-lens --calibrate   # drag a box over the Runeshape panel area
./app/poe2-lens-launch                   # run the overlay
```

Calibration is once. The launcher installs a desktop entry and starts the
app in a systemd scope because the global shortcuts portal refuses to bind
hotkeys for a process without an app id; running the binary directly works
except the hotkeys.

## Hotkeys

| Key | Action |
|-----|--------|
| F7  | Price the item under the cursor |
| F8  | Overlay on or off |

Two keys is the whole surface on purpose. Panel detection, focus pausing,
and price freshness are automatic; F8 is the one manual override, and it
stops the pipeline too, not just the drawing. KDE asks once to approve the
shortcuts on first launch (and once more after this change, since the
binding set changed).

## Configuration

Settings live in `~/.config/poe2-lens/config.toml`. The fields that matter:

- `league`: which economy to price against. Defaults to the current league.
- `refresh_minutes`: price refresh interval, default 10.
- `divine_threshold`: above this many divine, values display in divine
  instead of exalted.
- `tier_decent_ex`, `tier_good_ex`: the exalted cutoffs for the label
  colors.
- `panel_open_brightness`, `panel_close_brightness`: the brightness
  hysteresis that decides when a panel is on screen. The defaults came from
  measuring real frames (parchment reads about 156 to 168, open game world
  about 40 to 90) and should not need touching.

Diagnostics from every run land in `~/.cache/poe2-lens/last-run.log`, with
the previous run kept next to it. If a row reads wrong, that log plus the
`--headless` mode (prints priced rows to the terminal) is how you find out
why. There is also `scanimg <image.png>` to run the whole pipeline against a
screenshot offline.

## What it will not do

No memory reading, no injection, no automation. It looks at pixels, and the
only input it ever synthesizes is the single Ctrl+C you asked for by
pressing F7. Trade API requests are rate limited client-side to the limits
the API itself declares in its response headers, so it backs off before the
server ever has to tell it twice.

## Credits

- [PoeAncientsPriceHelper](https://github.com/pedro-quiterio/PoeAncientsPriceHelper)
  by pedro-quiterio. The Windows tool that proved this overlay was worth
  having. Its scan stabilization design, brightness hysteresis, and
  never-guess gem rule are ported here with their original constants.
- [poe.ninja](https://poe.ninja/poe2) for the price data.
- [Awakened PoE Trade](https://github.com/SnosMe/awakened-poe-trade) and
  [Exiled Exchange 2](https://github.com/Kvan7/Exiled-Exchange-2) for
  establishing the Ctrl+C price-check pattern the F7 flow follows.
- [RuneHelper](https://github.com/Denzeriko/RuneHelper) and
  [RuneshapePriceChecker](https://github.com/Barragek0/RuneshapePriceChecker),
  the two Windows tools for this same panel, whose approaches I studied
  before settling on per-row strips and learned templates.
- Fontin by Jos Buivenga (exljbris), the freeware face the game's UI uses,
  bundled for the labels.
- Currency icons are Path of Exile 2 art, property of Grinding Gear Games,
  used the way every community tool uses them.

Not affiliated with Grinding Gear Games.
