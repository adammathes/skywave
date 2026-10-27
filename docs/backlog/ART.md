# Art direction pass (after first play)

Owner feedback, 2026-10-27: "cockpits and interiors should be more colorful, vibrant, and probably need a
higher polygon count. Same with the cars and especially the non-player cars. In general the look is pretty
good but is still a little too spartan; for our target platforms we can up the poly count a little."

## New budgets (replace the M1 numbers in docs/RENDERING.md and docs/VEHICLES.md)
| | Was | Now |
|---|---|---|
| Player car exterior (LOD0) | 3–6k tris | 8–14k tris |
| Player car interior | 1.5–3k | 4–8k |
| Parked / NPC cars (LOD1) | ~2k | 3–5k (LOD0 for the nearest 8 within 25 m: full exterior at 8k) |
| Props | 300–1500 | 300–2500 (trees 600–1200, lamps 150) |
| Buildings | ≤ 2k | ≤ 3.5k (more sills, cornices, storefront depth, roof clutter) |
| Chunk vertex cap | 256k | 384k (web pool to 6M vertices after the per-building LOD) |

## Interiors (`vehicle::shape::interior`)
- Era palettes per car: 80s velour burgundy / tan / blue / grey two-tone; contrasting piping; woodgrain or
  brushed-metal dash applique band; colored carpet; chrome bezels on dials; dash top in a lighter tone.
- Geometry: hooded cluster with real bezel rings, stalks with knobs, HVAC vents with louvers, a center
  console with shifter boot and cup wells, door cards with armrest, speaker grille, window crank/switch,
  pull handle; seats with stitched bolsters and headrests; sun visors; rear-view mirror housing; seat belts.
- Instrument faces: colored dials (80s: green/orange glow, white needles with red tips), odometer window,
  warning-light strip (unlit), radio with a real display and preset buttons; all emissive at night.
- Warm cabin bounce: tint the cabin ambient by the interior palette so a red interior glows red at dusk.

## Cars (`vehicle::shape`)
- Two-tone paint and pinstripes as palette entries; chrome window surrounds; rocker trims; badges as small
  geometry; real door seams with handles; hood/trunk shut lines; sunroof option; vinyl top variant;
  whitewall tires; hubcaps vs alloys; tail-lamp lenses with reflector facets; reversing lamps; side marker
  lamps; mud flaps; roof racks on wagons; towing hitch on trucks.
- NPC cars draw from the full roster with era color distributions (80s: tan, white, burgundy, two-tone
  silver/blue, brown); dirt/wear tint variation per instance.

## Props and street
- Street clutter: newspaper boxes, phone booths, bus stops with benches and route signs, parking meters,
  mailboxes (already), fire-escape ladders on brick, roof HVAC units, water tanks downtown, awning poles,
  cafe tables on the strip, surfboards/bike racks on the beach, lifeguard tower, trash dumpsters in alleys.
- Vegetation: bougainvillea blocks on stucco, hedges with flowers, lawn edging, planters on the strip.

## Verification
- Preview sheets for every car (tools/shape-preview) and four fixed coast scenes before/after.
- Deck Medium preset must still hold 90 fps; budgets above assume the LOD and culling from the perf pass.
