# Scenarios

A **scenario** is the thing you pick on the title screen and share as a code: a kind of place, an era, the cars on the road, the radio, the light. People will want the place of their childhood. The procedural templates give the *kind* of place; the place compiler (M6) gives the *specific* one. Both use the same era layers, rosters and presets.

```
Scenario(
  id: "socal_beach_1984", title: "Pacific Shore, 1984",
  region: "california_coast", era: 1984, default_seed: 1,
  roster: "1980s_us", radio: "1984_socal", palette: "sunbleached_pastel",
  default_time: (18, 40), default_vehicle: "corsair_gtx", weather: Clear,
)
```

## Launch scenario
**`socal_beach_1984` — Pacific Shore, 1984.** Beach strip with neon and palms, a small downtown, a residential grid with a canal pocket, a highway through brown hills to a farm valley. Golden hour, yacht rock and soft rock, pastels and stucco. The test of the whole idea.

## Next two (M7, after the slice works)
**`chicago_1977` — Lake City, 1977.** Region template `great_lakes_city`: a flat grid city on a lake. Lakefront drive with a beach and harbor, a dense Loop with mid-century towers, elevated rail (the L as a prop system: steel structure over streets, stations, the rumble), brick three-flats and bungalows in neighborhoods, wide boulevards, expressways with sodium lights, industrial river corridor with bridges. Winter-adjacent light (low sun, long shadows), brown brick and limestone palette. Radio: soul, disco, AOR, Chicago blues presets.

**`north_shore_suburb_1986` — Lincolnwood-style, 1986.** Region template `north_shore_suburb`: a flat, pre-war grid suburb at the city edge. Brick bungalows and 50s ranches on 40-foot lots with alleys and detached garages, parkway elms and maples, strip malls and a shopping center with a big parking field, a brutalist tower hotel as the landmark, a six-lane expressway along one edge with a rail line beside it, forest preserve and golf course green, a Jewish deli, a diner, a cemetery, a high school with a track. Flat and frank light, brick-red and beige palette, mercury-vapor streetlights going sodium. Radio: 80s pop, new wave, classic rock, oldies presets. The procedural version evokes it; the place compiler on the real bounding box is the childhood version.

## Later ideas (write them down when they come)
`miami_strip_1986` (deco, neon, causeways), `desert_highway_1971` (motels, gas stations, nothing for miles), `seattle_1992` (hills, rain, ferries), `nyc_outer_borough_1979`, `tokyo_ward_2005` (narrow streets, vending machines, elevated expressway), `la_valley_1999`, `austin_2012`.

## Rules
- Real place names are facts and may be used in titles for compiled places. Procedural templates use invented names.
- Era is approximate and honest; the goal is a feeling, not a museum.
- Each scenario ships with a radio preset file of real public stations whose terms allow listening, plus folder hints for the user's own library; never audio.
