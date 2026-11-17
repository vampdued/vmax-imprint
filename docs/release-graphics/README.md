# Release graphics — Caption Studio v1.0.4

Generated with the VMAX kit's release pipeline (`vmax_release_v2.py`, brand
kit v1.1.0 — the pipeline lives in that kit's `release/` dir, not in this
repo; the zip he sent for Phase 1/2 lacks it, the v1.1.0 zip has it).

- Cover: the app icon — white VMAX interlock on a black tile, 1024×1024
  (`png/vmax-icon-white-1024.png` from the kit is the *white* tile, so the
  cover used here is that file with its RGB inverted, composited on black).
- Fonts: the pipeline's `chakra-plex` set — Chakra Petch Bold/SemiBold from
  this repo's `assets/fonts/`, plus IBMPlexSans-Medium instantiated from the
  variable font (`IBMPlexSans-Variable.ttf`, wght 500) with fontTools.
- Copy: `v1.0.4/release.json` (draft — tag, title, badges and foot are his to
  correct at tag time; re-run to regenerate).

Result: 84 assets (3 templates × 1 font set × 4 themes × 7 sizes), 76 passed
QA first try, 8 passed after the pipeline's auto-fix, 0 failed. On the tight
`type` templates (4x5, 1x1) the auto-fix dropped the credits row and then the
film row to satisfy margins — those assets carry less copy by design.
Contact sheets + `report.json` sit alongside the assets.

Re-run:

```sh
python3 vmax_release_v2.py \
  --cover cover-black.jpg \
  --release release.json \
  --out docs/release-graphics/v1.0.4 \
  --contact-sheets
```

(Fonts go in the pipeline's `fonts/` dir: ChakraPetch-Bold.ttf,
ChakraPetch-SemiBold.ttf, IBMPlexSans-Medium.ttf.)
