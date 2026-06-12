# Xbox One controller assets

These SVG sprites and the accompanying `template.css` come from
[e7d/gamepad-viewer](https://github.com/e7d/gamepad-viewer), commit
`master` (May 2026), licensed **MIT**.

Originally built for in-browser DOM rendering: each input is a
sprite-sheet stacked into a `750×630` canvas, with positions
defined in `template.css`. Reused here as the visual base for the
Xbox 360 / Xbox One mapping preview in WiiPair's UI.

## File map

| File | Role | Size |
|---|---|---|
| `base-black.svg` / `base-white.svg` | Controller body, two colourways | 750×630 |
| `trigger.svg` | LT/RT — clip-path animated by trigger value | 89×122 |
| `bumper.svg` | LB/RB — overlay (opacity 0/1) | 170×61 |
| `buttons.svg` | A/B/X/Y face buttons sprite-sheet (4 buttons × 2 states) | — |
| `dpad.svg` | D-pad sprite-sheet (4 directions) | 110×111 |
| `start-select.svg` | Start/Back overlay sprite-sheet (2 states each) | 66×33 |
| `stick.svg` | Analog stick sprite-sheet (2 states) | 170×83 |
| `disconnected.svg` | Full-canvas overlay shown when no controller is detected | 750×630 |
| `template.css` | Reference layout (positions/dimensions of every sprite) | — |

## Attribution

Original work © e7d, MIT-licensed. See the upstream repository for
the full LICENSE text.
