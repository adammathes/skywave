# Buildings: a facade grammar per era and zone

Owner note 4(b), 2026-10-27 (`docs/backlog/NOTES.md`): the buildings look fake; more realistic
buildings, no fake text. Sources and what we may do with them: `docs/adr/0005-reference-sources.md`.
Budgets: `docs/backlog/ART.md` (≤ 3.5k triangles per building).

## Where we are
`crates/world/src/buildings.rs` already plans and builds facades: a floor grid, storefront bays
with doors, awnings (striped, solid, flat canopy), sign bands, cornices and parapets, styles with
their own look (dingbat, bungalow, ranch, tract, motel, warehouse, glass tower, barn), step-backs
and roof clutter (vents, AC boxes, a stair bulkhead, water tanks). `crates/world/src/signs.rs`
letters invented names in a 5-pixel block font.

What reads as fake:
- Every storefront is the same: one glass rectangle per bay, one sign band, the same proportions in
  every zone. Real commercial fronts have a bulkhead, display glass, a transom, piers between bays,
  and a sign band whose height and depth say what decade the front was remodeled in.
- The lettering. A 5-pixel block font at sign scale reads as placeholder text, and two-syllable
  names ("TAMLEY'S DONUTS" on every corner) read as generated. Real signs are mostly a picture
  and one or two words in a letterform with character.
- Rooflines are a box with a lip. No false fronts, no stepped or shaped parapets, no mansard
  canopies, no bracketed cornices.
- Missing the things that make a street lived in: fire escapes, window AC units, back porches,
  security grilles, ghost signs, parking lots with stripes and wheel stops.
- The same rules everywhere: a 1977 brick street and a 1985 beach strip come out alike.

## The grammar
A building's look is a pure function of `(building, era, zone, style, seed)`, as today, but the
rules move from code branches into data: one RON grammar file per era and region template,
`assets/grammar/<template>_<era>.ron` (e.g. `california_coast_80s.ron`,
`great_lakes_city_70s.ron`), loaded by `app` and handed to `world` as plain structs (ADR 0002).
Code keeps the geometry builders (a bay, an awning, a fire escape); the grammar chooses which, in
what proportions, how often. No trait objects, no rule interpreter: a grammar is tables of ranges
and weights read by plain loops.

```
Grammar      { era, template, styles: Vec<StyleRules>, signs: SignRules, lots: LotRules }
StyleRules   { style: StyleTag, zones: ZoneMask,
               massing:  floors, floor_h_ground, floor_h_upper, setbacks, roof kinds + weights,
               base:     bay_width, pier_width, bulkhead_h, glass_h, transom_h, recessed_entry_p,
                         ground kinds (storefront | lobby | garage | wall | door) + weights,
               body:     window kinds (punched | paired | ribbon | bay | picture) + weights,
                         window w/h, sill and lintel depth, belt courses,
               top:      parapet kinds + weights, parapet_h, cornice depth, coping color,
               addenda:  fire_escape_p, window_ac_p, roof_units per 100 m², water_tank_p,
                         grille_p, ghost_sign_p, back_porch_p, gutters_p,
               palette:  walls, trim, glass tint, awning cloth, roof }
SignRules    { band_h, band_depth, blade_p, pole_sign_p, rooftop_sign_p, lit_box_p, neon_p,
               pictogram weights per trade, wordmark styles + weights, lexicon id }
LotRules     { front_lot_p by style, stall w/l, aisle, angle, wheel_stop_p, pole spacing,
               island_p, curb cuts per frontage }
```

Every range has a source comment (ADR 0005, "Fit"): a HABS drawing, a Library of Congress photo ID,
a FEMA or city-dataset distribution. Until then the numbers below are starting values, marked so.

A facade is three bands, split independently:
- **Base** (ground floor, 4.0–5.0 m): a run of bays separated by piers. A bay is one of
  storefront, lobby door, garage, wall, or residential door. A storefront bay is, bottom up:
  bulkhead (0.4–0.6 m, tile or panel), display glass (to 2.6–3.0 m) with thin mullions, transom
  (0.5–0.9 m, prism glass in pre-war fronts, often panelled over in remodels), then the sign band.
  Entry centered or at one side, recessed 1.0–1.8 m in pre-war fronts, flush in 70s/80s ones.
- **Body** (upper floors): window kinds per style, laid out on the bay grid above the base so
  windows line up with the piers below (the one rule that most makes a facade read as built).
- **Top**: parapet, cornice or roof. Corner buildings may turn the corner with a chamfer and a
  corner entry.

## Signage: pictograms and invented wordmarks
No real names, no placeholder text (no "SHOP", "STORE", "NAME", lorem, or a trade word repeated as
filler), no fonts we do not own. A sign is a pictogram, a wordmark, or both.

**Pictograms** (new, `world::signs`): about 32 flat shapes drawn as small 2D polygons, extruded a
few centimeters, in the era palette, lit as neon outline or backlit box at night. Generic objects
only, never a mark: donut, coffee cup, burger, taco, pizza slice, ice cream cone, hot dog, fish,
martini glass, beer mug, surfboard, wave, palm, sun, star, arrow (the motel arrow), record, TV set,
camera, film reel, scissors and comb, shoe, key, wrench, tire, tooth, mortar and pestle, shirt on
a hanger, book, bicycle, bowling pin, flower. Never a red cross on white (a protected emblem).

**Wordmarks**: one or two words from a closed lexicon, lettered in a stroke style.
- Lexicon (data, reviewed by a person, never generated free-form): invented surnames and given
  names built from phonotactic tables so they read as names, not syllable soup ("Dalvey's",
  "Orlan", "Kessick"); generic descriptors (Sunset, Harbor, Surf, Lakeshore, Corner, Golden,
  Coast, Northside, Avenue, Royal, Star); trade words (Donuts, Liquor, Records, Cleaners,
  Cocktails, Tacos, Motel, Bowl, Tires). Forms: `Name's`, `Name's Trade`, `Descriptor Trade`,
  `Trade` alone, plus a secondary line (`Cocktails`, `Open`, `Vacancy`, `Since 19xx` with a year
  before the era). A name appears at most once per few blocks. Since we cannot test against every
  real brand without writing real brands into the repo, the lexicon is closed: only reviewed
  words, combined by rule.
- Letterforms (ours, as stroke polylines with a width): block capitals (the current font, redrawn
  at higher resolution with proper spacing), condensed sans, a connected script for neon, a slab
  serif for painted signs. Neon is a tube along the stroke; a backlit box is dark strokes on a
  lit panel; channel letters are extruded strokes; painted signs are flat color on brick.
- Sign carriers: flush band over the storefront, projecting blade sign (pre-war Chicago, with
  bulbs or neon), pole sign at a strip-mall or motel entrance, rooftop letters on a frame, a
  ghost sign (faded painted wordmark on a brick side wall), window lettering (a short trade word),
  a sandwich board.
- Night (feeds note 4(a)): neon and boxes switch on at dusk with a per-sign delay; a share of shops
  are closed and dark; one tube in a few signs flickers.

## 1980s SoCal beach town (`california_coast`, Eighties) — first
Low, stuccoed, sun-faded, parking in front. Sign-heavy along the strip, quiet behind it.

| Zone / style | Massing | Base and signs | Top and addenda |
|---|---|---|---|
| `Strip` / `StripMall` | 1 storey, 4.5–5.5 m, depth 18–25 m, set back 20–35 m behind a front lot | Bays 6–9 m; full-height glass, aluminum frames, flush entries; continuous mansard shingle canopy (0.9–1.5 m tall, projects 2–3 m on posts) or flat stucco canopy; channel letters or backlit boxes on the fascia; one pole sign at the curb cut with 3–6 tenant panels (pictograms plus one word each) | Flat roof, packaged rooftop units 2–4 per 1000 m²; front lot (below) |
| `Strip` / `StuccoBox` (older strip fronts) | 1–2 storeys at the sidewalk, frontage 7–15 m | Remodeled pre-war fronts: storefront glass, a deep sign band 1.2–1.8 m; striped canvas or solid awning (projects 1.2–1.8 m, min 2.4 m clearance); neon script wordmark | Parapet, flat or stepped; false front on 1-storey; roof units visible |
| `Strip` / `Deco` | 2 storeys, corner lots | Terrazzo bulkheads, curved glass at the corner, blade sign | Stepped parapet with speed-line bands, pastel repaint |
| `Strip` / `Motel` | 2 storeys, L or U around a court, lot parking | Door + window + through-wall AC per room, walkway on posts | Rooftop or pole sign with arrow pictogram and `Vacancy`; pool fence |
| `Beach` | 1 storey stands, lifeguard towers, a pier shack | Roll-up service windows, painted menu boards of pictograms | Shed roofs, surfboard racks |
| `Commercial` | 1–3 storeys, offices and banks | Ribbon windows, brown-tinted glass, planters | Flat roofs, roof units, a few parking structures |
| `Residential` / `Dingbat` | 2 storeys over open carports | Stucco, flat front, script address or name on the wall (invented) | Flat roof, window AC 10–20% of windows |
| `Suburb` / `Ranch`, `Tract70s` | 1 storey, 2-car garage | Garage doors facing the street | Low hips, tile or shake, swamp coolers rare |

Palette: sun-faded pastels (salmon, mint, sky, sand, white), terracotta, brown anodized frames,
orange and brown awning stripes. Starting values; fit from the Margolies archive and NHAP 1980s
aerials.

## 1970s Chicago (`great_lakes_city`, Seventies) — next
Brick, dense, walk-up, alleys. Lot module: 25 ft × 125 ft (7.6 × 38 m); buildings fill the width.

| Zone / style | Massing | Base and signs | Top and addenda |
|---|---|---|---|
| Arterial `Commercial` / `Brick` (storefront with flats above) | 2–3 storeys, frontage 7.6–23 m (1–3 lots), at the sidewalk | Storefront bays with tile bulkheads, recessed entries, transoms (often panelled over), a separate residential door to the stairs; projecting blade signs with bulbs or neon; canvas awnings, some rolled up; painted window lettering | Flat roof behind a parapet with stone coping; pressed-metal or brick corbel cornice on older blocks; ghost signs on exposed side walls; fire escape on the side or rear |
| `Residential` / three-flat (`Brick` with greystone face) | 3 storeys + raised basement, 6–7 m wide | Front stoop, bay window on one side, entry on the other | Flat roof with a short parapet; rear wooden stair porches across the back; window AC 15–30% of windows; alley garage |
| `Residential` / courtyard building | 3 storeys, U around a landscaped court, 30–60 m frontage | Entries off the court, limestone trim | Parapet with decorative caps; fire escapes on the court; AC units |
| `Residential` / bungalow (`Craftsman` variant) | 1.5 storeys, brick, 7.6 m lot | Front porch or bay | Hipped roof, dormer |
| `Downtown` / `Brick`, `Glass` | 6–30 storeys | Lobbies, bank fronts, rolldown grilles on some shops | Water tanks on older towers, bracketed cornices, step-backs; fire escapes on alley sides |
| `Industrial` / `Warehouse` | 2–6 storeys loft or 1 storey sawtooth | Loading docks with canopies, rail spurs | Water tanks, painted wall signs, smokestacks |

Palette: brown, red and yellow ("common") brick, limestone and greystone, black iron, dark green
and maroon trim, faded canvas. Starting values; fit from HABS Chicago surveys, DOCUMERICA (1970s
Chicago), Sanborn sheets and the city footprint `stories`/`year_built` distribution.

## 1980s Chicago (`great_lakes_city`, Eighties) — after
Same stock, a decade on. Overrides only: backlit plastic box signs replace many blade signs;
vinyl and aluminum awnings; rolldown security grilles (closed at night, a share of shops);
mansard shingle canopies on remodeled fronts; corner mini-malls (1 storey, L-shaped, parking in
front) on some arterial corners; more window AC; fewer neon signs lit. A grammar file that names
only the differences from the 70s file (`base: great_lakes_city_70s`).

## Shared pieces
- **Fire escapes**: steel; a platform per floor 1.0–1.2 m deep spanning two windows, rails 1.0 m,
  stairs between platforms, a counterweighted drop ladder over the sidewalk or alley. Placed on
  brick buildings of 3+ storeys, alley and side walls first, the front of some walk-ups. Shadows
  matter: thin geometry, one draw per building, no alpha.
- **Window AC**: a box 0.65 × 0.4 × 0.55 m projecting 0.3 m below a raised sash, beige or grey,
  placed per window by `window_ac_p` with a per-building bias, so some buildings have many and
  some none (an even scatter looks generated). Through-wall units under motel windows. Drip stains are vertex color.
- **Rooftop units**: packaged units with a duct elbow and a curb, exhaust fans, plumbing vents,
  skylights on strip malls; counts by roof area. Parapets hide most of them from the street.
- **Rooflines**: flat with parapet (plain, stepped, shaped, false front), bracketed cornice,
  mansard canopy, hip, gable, sawtooth, gambrel (barns); coping and flashing as a trim color.
- **Parking lots** (a parcel feature, `LotRules`): 90° stalls 2.6–2.75 × 5.5 m with 7.3 m two-way
  aisles, or 60° stalls with 5.5 m one-way aisles; white or yellow stripes; concrete wheel stops;
  light poles 9–12 m every 30–40 m; planter islands at row ends with a palm (SoCal) or nothing
  (Chicago); one or two curb cuts per frontage; about one stall per 25 m² of floor area in a
  strip mall. Chicago: alley garages and small fenced lots behind, chain-link and a sign
  pictogram. Parked cars from the era roster, a share that come and go (note 4(a)).
- **Security and wear**: rolldown grilles, burglar bars on ground-floor windows, faded paint
  bands, a few boarded upper windows in the 70s, stained parapets; all vertex color or simple
  geometry.

## Tasks
Each is one branch and one PR into `main`, sized to merge on its own. Branch names `bldg/<slug>`.
Visual tasks attach before/after screenshots of the four fixed coast cameras and move golden hashes in their own commit.

| # | Task | Files | Depends |
|---|---|---|---|
| B1 | **Done.** `CREDITS.md` at the root: the format from ADR 0005 and, at its top, how to record a fitted number and its source | `CREDITS.md` | — |
| B2 | **Done** (as built: `docs/WORLD_GENERATION.md`, "Facade grammars"; fields move in as later tasks need them). Grammar types in `world` (`no_std`, serde), RON loader in `app`, `california_coast_80s.ron` that reproduces today's look exactly: no hash moves. Tests: loader round-trip, current goldens unchanged | `world::grammar`, `app` loader, `assets/grammar/` | — |
| B3 | Fit pass, numbers only: measure 20+ public-domain references per scene (Margolies, DOCUMERICA, HABS, NHAP) into the SoCal grammar with source comments | `assets/grammar/` | B2 |
| B4 | Three-band facade and the storefront bay (bulkhead, glass, transom, piers, recessed entry); upper windows aligned to the base bays | `world::buildings` | B2 |
| B5 | Pictogram library: 32 shapes as 2D polygons, extrude and neon-outline builders, a preview sheet in `tools/shape-preview`; tests for triangle budget and closed outlines | `world::signs`, tools | — |
| B6 | Lexicon and wordmarks: phonotactic name tables, descriptors, trades, forms; stroke letterforms in four styles replacing the 5-pixel font on shop signs; uniqueness within a radius | `world::signs`, `assets/grammar/lexicon.ron` | B2 |
| B7 | Sign carriers: band, blade, pole, rooftop, window lettering, ghost sign; night switching and flicker | `world::buildings` (sign fns), `world::procgen::detail` | B5, B6 |
| B8 | Awnings and canopies: shed (striped, solid), bullnose, mansard shingle canopy, flat stucco canopy, aluminum; per-era weights | `world::buildings` (awning fns) | B4 |
| B9 | Rooflines and rooftop units: parapet kinds, bracketed cornice, false front; packaged units with ducts by roof area | `world::buildings` (top, roof clutter) | B2 |
| B10 | Parking lots: `LotRules` layout on front and rear lots, stripes, wheel stops, poles, islands, curb cuts; parked-car placement on stalls | `world::procgen` (parcels/props), `world::mesh` ground | B2 |
| B11 | Window AC, through-wall units, grilles, burglar bars, wear bands | `world::buildings` (addenda) | B4 |
| B12 | SoCal beach town pass: tune the grammar until the four coast cameras read as a 1985 beach town; owner look | `assets/grammar/` | B3–B11 |
| B13 | Fire escapes and rear wooden porches | `world::buildings` | B4 (needed by Chicago) |
| B14 | `great_lakes_city_70s.ron`: brick storefront-with-flats, three-flat, courtyard building, bungalow, loft; Chicago fit pass (HABS, DOCUMERICA, Sanborn, city footprint statistics) | `assets/grammar/` | M7 template, B13 |
| B15 | `great_lakes_city_80s.ron` as overrides of the 70s file; grammar inheritance (`base:`) | `world::grammar`, `assets/grammar/` | B14 |

B4, B8, B9, B11 and B13 all touch `world::buildings`; run them one after another, not side by
side (`docs/HANDOFF.md`). B5 and B6 can run in parallel with them. B10 is independent.

## Acceptance
- Same seed, same grammar → bit-identical meshes on every platform; goldens move only on purpose.
- ≤ 3.5k triangles per building and the chunk caps in `docs/backlog/ART.md`; Deck Medium holds
  90 fps on the coast.
- No sign carries placeholder text; every word on a sign comes from the reviewed lexicon.
- Every fitted number in a grammar file names its source.
- The owner, driving the strip at dusk: does it look like a real place?
