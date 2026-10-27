# Scenarios

A **scenario** is the thing you pick on the title screen and share as a code: a kind of place, an era, the cars on the road, the radio, the light. People will want the place of their childhood. The procedural templates give the *kind* of place; the place compiler (M6) gives the *specific* one. Both use the same era layers, rosters and presets.

As built (`assets/scenarios/<id>.ron`, read by `crates/app/src/scenario.rs` with
`deny_unknown_fields`; the World page lists them in the fixed order of `scenario::BUILTIN`):
```
Scenario(
  id: "socal_beach_1984",              // the file stem; `?scenario=` / `--scenario`
  title: "Pacific Shore", year: 1984,  // what the World page's card says
  kind: "Beach town",                  // the kind of place, on the card
  region: "california_coast",          // assets/regions/<id>.ron
  grammar: "california_coast_80s.ron", // assets/grammar/<file>
  radio: "1984_socal",                 // assets/radio/<id>.ron: the dial starts on it
  palette: "socal_1985",               // a world::Palette
  default_time: (17, 40),              // the clock a world starts at, running
  default_vehicle: "gannet_gt",      // the car it starts in unless one was picked
  years: (1979, 1987),                 // the era: cars of these years are everyday cars
  heroes: [],                          // rare-by-age cars kept on the road (weight >= 0.05)
  stub: false,                         // optional: a world still being built ("coming")
)
```
Validation: years in order and the year within them, a known region, grammar, palette and preset, a
default car no newer than the years, heroes that are built-in cars. The era filter and its weights are in
`docs/VEHICLES.md` (`year`). Each scenario has a 320×200 still, `assets/stills/<id>.png` (under 60 KB,
committed; rendered with `tools/screenshot` and cropped clear of the HUD, see `assets/stills/README.md`).

**Switching worlds.** The pause menu's World page (`docs/CONTROLS_INPUT.md`) shows a card per scenario;
picking one saves it as the `scenario` setting and the platform reloads the game in it: the session's scene
moved to that world's procedural place, its start and default time (`SceneSetup::for_world`), the settings
kept, the radio off; the old app and renderer are dropped whole. A session starts in the world last picked
unless its scene names a place or a world (`?scenario=<id>` on the web, `--scenario <id>` natively; an id
that is not built in starts the first world and says so). There is no title screen yet: the World page is
reached from the pause menu (and `?menu=worlds` opens the web build on it).

## Launch scenario
**`socal_beach_1984` — Pacific Shore, 1984.** Beach strip with neon and palms, a small downtown, a residential grid with a canal pocket, a highway through brown hills to a farm valley. Golden hour, yacht rock and soft rock, pastels and stucco. The test of the whole idea.

## Next two (M7, after the slice works)
**`chicago_1977` — Lake City, 1977.** Region template `great_lakes_city`: a flat grid city on a lake. Lakefront drive with a beach and harbor, a dense Loop with mid-century towers, elevated rail (the L as a prop system: steel structure over streets, stations, the rumble), brick three-flats and bungalows in neighborhoods, wide boulevards, expressways with sodium lights, industrial river corridor with bridges. Winter-adjacent light (low sun, long shadows), brown brick and limestone palette. Radio: soul, disco, AOR, Chicago blues presets.

**`north_shore_suburb_1986` — Lincolnwood-style, 1986.** Region template `north_shore_suburb`: a flat, pre-war grid suburb at the city edge. Brick bungalows and 50s ranches on 40-foot lots with alleys and detached garages, parkway elms and maples, strip malls and a shopping center with a big parking field, a brutalist tower hotel as the landmark, a six-lane expressway along one edge with a rail line beside it, forest preserve and golf course green, a Jewish deli, a diner, a cemetery, a high school with a track. Flat and frank light, brick-red and beige palette, mercury-vapor streetlights going sodium. Radio: 80s pop, new wave, classic rock, oldies presets. The procedural version evokes it; the place compiler on the real bounding box is the childhood version.

## Third: the early 2000s (M8)
**`inland_basin_2001` — Inland Basin, 2001.** Region template `socal_basin`: the flat inland grid of a big
Southern California city at the turn of the century, inspired by the import street-racing culture
of the era and the films and shows that mythologized it (an influence, not a copy: no titles, no
names, no scenes reproduced). Wide industrial boulevards with four-lane straights, warehouse
districts with roll-up doors and chain-link, mini-malls with backlit box signs, stucco boxes,
palms on wide medians, a concrete flood-control channel you can drive down, a tiled downtown
tunnel, freeway overpasses and an on-ramp loop, a rail line with a grade crossing, an airfield
drag strip at the edge, a hills overlook at night with the city grid lit below, a desert highway
leaving town past truck stops. Night is the default time: sodium-orange street lamps, neon
underglow on the tuner cars, a lit race start line with a crowd that steps aside.

Era rules: the roster is filtered by year. A scenario has `years: (min, max)`; a car has a
`year`. Cars newer than the scenario's `max` never appear, in traffic, parked or on the vehicle
page. Older cars appear with weights that fall off with age (a 1984 coupe is rare but present in
2001; a 1969 muscle coupe is a hero car), so the 2000s world has its own cars plus a thinner slice
of the 80s roster, and the 80s world never sees a 2000s car.

Hero cars (`docs/backlog/INLAND2001.md`): a late-90s Japanese inline-six twin-turbo grand tourer; an
all-wheel-drive twin-turbo Japanese coupe with four round tail lamps; a late-60s big-block American
muscle coupe with a hood-mounted supercharger; a 90s front-drive Japanese tuner coupe with a tall
wing and a body kit; a rotary sports coupe; a Japanese all-wheel-drive rally sedan with a hood
scoop; a German compact sedan with a body kit; a mid-90s American V8 sports car. Traffic cars of
2001: a jellybean sedan, a mid-size SUV, a minivan, a full-size pickup, a compact hatch, a box
truck and semis on the freeway.

Set pieces (places, not missions; nothing can be hurt): the night street race (a four-wide start
line painted on an industrial boulevard, spectator cars and a crowd that clears, three opponent
cars that run the course as NPC racers when you line up), the truck run (a convoy of semis on the
desert highway at dawn you can run alongside), the train race (a grade crossing whose gates drop
as the train comes), the channel run (ramps into the flood channel and a long run under the
bridges), the tunnel, the drag strip (a lit quarter mile with a tree), the hills overlook, the
crew garage (a warehouse with the hero cars parked and a lit sign), a taco stand lot with lowriders.

"Change worlds": a World page listing every scenario as a card (title, year, a still, the cars and
stations it ships with); picking one reloads the place and persists; `?scenario=` on the web. Built (see
the format above), in the pause menu. Until the basin's template, grammar, palette and preset land,
`la_basin_2001` is a stub on the coast's, marked "coming" on its card.

## Later ideas (write them down when they come)
`miami_strip_1986` (deco, neon, causeways), `desert_highway_1971` (motels, gas stations, nothing for miles), `seattle_1992` (hills, rain, ferries), `nyc_outer_borough_1979`, `tokyo_ward_2005` (narrow streets, vending machines, elevated expressway), `la_valley_1999`, `austin_2012`.

## Rules
- Real place names are facts and may be used in titles for compiled places. Procedural templates use invented names.
- Era is approximate and honest; the goal is a feeling, not a museum.
- Each scenario ships with a radio preset file of real public stations whose terms allow listening (ad-free, listener-supported or college), plus folder hints for the user's own library; never audio.
- Influences are described generically: no film, show, game, studio or console titles, no real car names. Invented place names must also not be the names of fictional places from films, comics or games (a review caught one and it was renamed). A scenario may evoke an era's screen culture through its places, light and cars, never by reproducing a scene, a title, a line or a logo.
