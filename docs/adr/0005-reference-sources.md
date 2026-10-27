# ADR 0005: Reference and data sources for places and buildings

Status: proposed, 2026-10-27

(Numbered 0005 because 0004 is the cpal ADR.)

## Context
The owner, after the first play: the world feels dead and the buildings look fake; find sources to
reuse or synthesize from (`docs/backlog/NOTES.md`, note 4). Two consumers want outside data: the
place compiler (M6, `docs/PLACES.md`), which turns real-world data into a Place skeleton, and the
procedural facade grammar (`docs/backlog/BUILDINGS.md`), which needs real-world proportions.
`CLAUDE.md` fixes the constraints: BSD-3-Clause code, no copyrighted assets, no ODbL, nothing
shipped that we do not own or that is not public domain / CC0.

Three ways a source can be used, from strictest to loosest:
- **Ship**: bytes from the source (or a database derived from it) end up in the repo, the binary or
  a Place file we distribute. Allowed only for public domain (US federal works) and CC0.
- **Fit**: we measure the source offline and keep only numbers we choose (a range of storefront
  widths, a distribution of floor counts, a palette of five colors). The numbers are facts, written
  by hand into RON templates with a provenance comment; the source is never in the repo.
  Allowed for public domain, CC0 and permissively licensed sources (CC BY with credit in
  `CREDITS.md`). Not for share-alike databases (ODbL), so that nobody has to argue whether a
  template is a Produced Work.
- **Consult**: a person or an agent looks at it to understand how something is built. Nothing is
  traced, sampled or measured into the repo. Allowed for anything we may lawfully view.

## Decision
Use the sources below in the mode their row allows, and only that mode. Every shipped or fitted
source gets a line in `CREDITS.md` (source, license, date retrieved, what we took). A Place file
records the same in its provenance header. When a row says "verify", the PR that first uses the
source quotes the license text it found and the date.

### Elevation, water, coastlines
| Source | License | Mode | Feeds |
|---|---|---|---|
| USGS 3DEP (1/3 arc-second and 1 m DEMs, lidar point clouds) | US federal work, public domain | Ship | `placec` terrain heights, resampled to the Place's 4 m grid |
| NOAA coastal DEMs (CUDEM, tsunami DEMs; topo-bathy) | Public domain | Ship | Shoreline and near-shore depth where 3DEP stops at the water |
| NOAA Continually Updated Shoreline (CUSP), NOAA ENC coastline | Public domain | Ship | Coast polygon → `WaterBody { kind: Ocean }` |
| USGS National Hydrography Dataset (NHD/NHDPlus HR) | Public domain | Ship | Rivers, lakes, canals |
| Natural Earth (1:10m vectors, rasters) | Public domain (dedicated) | Ship | Far-field context only: what is beyond the map edge, distant ridges for the horizon (NOTES note 3) |
| Copernicus DEM GLO-30 | Free with attribution (own license) | Fit (verify before ship) | Non-US places only; not needed for the US scenarios |

Procedural places may fit their region templates' elevation profiles (beach width, ridge heights,
coastal-plain slope) to 3DEP transects of real coasts. The template keeps numbers, not terrain.

### Road networks
**OpenStreetMap is out**, for shipped data and for fitting. It is ODbL: any Place built from it is a
Derivative Database that must be offered under ODbL, and `CLAUDE.md` forbids ODbL. That rules out:
OSM extracts and planet files; everything derived from them (Overture Maps `transportation`,
`base` and `divisions`, which are ODbL; vector tile sets built on OSM; routing graphs exported from
OSM tools); tracing from OSM-based maps; and the "ship it as a separate ODbL data file" plan in the
old `docs/PLACES.md` sketch, which this ADR replaces. Agents may not use OSM to check their work
either; a place compiled from clean sources is compared against those sources.

What we use instead:
| Source | License | Mode | Feeds |
|---|---|---|---|
| US Census TIGER/Line (edges, roads, rails, water, landmarks) | Public domain | Ship | `placec` road graph: geometry, `MTFCC` road class → `RoadClass` (S1100 → Highway, S1200 → Arterial, S1400 → Local, S1730 → Alley), real street names for compiled real places, one-way where marked |
| Earliest TIGER vintages (1990/1992 TIGER, the 2000 TIGER) | Public domain | Ship | Closest-to-era street layout for 70s/80s places: drop roads that do not appear in the earliest vintage |
| USGS Historical Topographic Map Collection (all editions since 1884, scanned GeoTIFFs) | US federal work, public domain | Ship (verify per sheet) / Consult | Era check: a 1970s or 1980s 7.5′ quad shows that decade's roads, rail, piers and the built-up tint. Digitizing roads from it by hand is allowed |
| State DOT road inventories (e.g. HPMS submissions) | Varies by state | Fit (verify) | Lane counts, speed limits, widths |

TIGER has no lanes or widths. `placec` derives them from `MTFCC` plus the zone (a TIGER S1200 in
`Strip` is a four-lane arterial), and the shared detailing pass does the rest, as for procedural
places.

### Building footprints and heights
| Source | License | Mode | Feeds |
|---|---|---|---|
| FEMA USA Structures (footprints > 450 sq ft with occupancy type and height where known; built with ORNL and USGS) | US government work (data.gov lists the usa.gov government-works terms); built partly from commercial imagery and parcel data | Fit; Ship only after verifying the state's release terms | `placec` footprints and `BuildingUse`; facade-grammar statistics (footprint area, depth/frontage by zone) |
| Microsoft US Building Footprints, Global ML Building Footprints | ODbL (data); code MIT | Out | Same problem as OSM |
| Google Open Buildings | CC BY 4.0 or ODbL, user's choice | Out for us in practice | No coverage of the US; CC BY would allow Fit elsewhere |
| Overture Maps `buildings` | ODbL (it merges OSM) | Out | — |
| City and county open-data portals (Chicago Data Portal building footprints with `year_built` and `stories`; Los Angeles County LARIAC footprints with heights; assessor parcels with year built) | Per-portal terms of use, not a standard license; some datasets also posted under MIT or public domain dedications; older terms allowed the city to demand removal | Fit after reading the dataset's own terms; Ship only when the dataset itself carries PD, CC0, MIT or CC BY | Floor counts, `built_year` for the era layer (drop newer buildings, backfill the lot), lot widths, setbacks per zone |
| Sanborn fire insurance maps, Library of Congress (pre-1964 sheets in the public domain) | Public domain where marked "no known restrictions" | Consult / Fit | Pre-war Chicago: footprints, storeys, material (brick, frame, stone) per building; three-flat and storefront-row proportions |
| Census ACS "year structure built", "units in structure" by tract | Public domain | Fit | Era mix per zone: how many 1920s brick flats versus 1960s tract houses a 1977 neighborhood had |

Heights: FEMA height where present, else `stories × 3.3 m` from a city dataset, else the zone's
fitted distribution. Never from imagery we do not own.

### Land use and zoning
| Source | License | Mode | Feeds |
|---|---|---|---|
| USGS/MRLC National Land Cover Database (NLCD, 30 m; 1985 onward in the Annual NLCD series) | Public domain | Ship | `placec` zone map seed (developed low/medium/high, forest, cultivated, water); era-correct for 1985 |
| USGS historical land use/land cover (GIRAS, 1970s–80s) | Public domain | Ship | Zone map for 70s places |
| City zoning layers (Chicago, Los Angeles and others) | Per-portal terms | Fit (verify) | Zone boundaries and which zones front arterials; zoning setbacks and height limits as template numbers |
| Census urban areas, block groups (TIGER) | Public domain | Ship | Density → `Downtown`/`Residential`/`Suburb` weights |

### Photographs for proportions (Consult and Fit only; we ship no imagery)
We look at photographs to learn proportions, rhythms, materials and colors, then write numbers and
rules by hand. Nothing is traced, projected or textured from a photo, even a public-domain one:
our look is vertex-colored geometry (`docs/RENDERING.md`), and a traced facade is a copy of one
building rather than a grammar.

| Collection | Rights | Why it matters |
|---|---|---|
| Library of Congress HABS/HAER/HALS (measured drawings and large-format photos) | Public domain, "no known restrictions" | Measured drawings with dimensions: storefront bays, cornice heights, three-flat bays, fire escapes, warehouse loading docks |
| Library of Congress, John Margolies Roadside America archive (1970s–2000s color slides) | Rights released to the public domain by the Library; artworks shown in a photo may still be copyrighted | The best period reference for roadside commercial buildings, motels, diners and sign forms. Study sign *shapes*, never copy a sign's lettering or mark |
| National Archives DOCUMERICA (EPA, 1971–1977) | US federal work, public domain | 1970s street life in Chicago, Los Angeles and beach towns: storefronts, parking, signage density, cars at the curb |
| Library of Congress Prints & Photographs (FSA/OWI, Highsmith, others marked "no known restrictions") | Public domain where marked | Pre-war Main Street proportions that survive into the 70s and 80s |
| USGS aerial photography: NHAP (1980–1988), NAPP (1987–2007), earlier single frames on EarthExplorer | Federal; verify NAPP citation terms | Roof clutter, parking lot layouts, lot coverage, tree cover for 80s SoCal and 70s Chicago, seen from above |
| Wikimedia Commons, filtered to `PD-USGov`, `PD-self`, `CC0` | Per file | Gap filler. Check the file's own license tag; skip CC BY-SA entirely (share-alike), and skip any file whose subject is a logo or artwork |
| Smithsonian Open Access, NYPL public-domain collections | CC0 / public domain | Postcards and photos of commercial streets, 1930s–1970s |

What a contributor writes down from a photo: dimensions (bay width, sill height, sign-band
height, awning projection, parapet height), counts (bays per frontage, windows per bay),
materials and their colors as a small palette, and which elements co-occur (a deep canopy implies
a parking lot in front). A fitted number cites the collection and item ID in a RON comment.

### Period street views we may look at but never copy
Period films and television, commercial photo books, stock photo libraries, postcards still in
copyright, modern street-level imagery services, real estate listings, news archives. Looking at
them to get a feel for a street is fine. Taking measurements, colors, sign designs, lettering,
names or layouts from them into the repo is not. Their terms usually forbid derivative extraction,
and the images are copyrighted. Real business names, logos and wordmarks seen in any photo, public
domain or not, never enter the game (`CLAUDE.md`).

### Shippable models and textures (CC0)
Our meshes are generated in code and vertex-colored; there is no glTF loader and no texture
pipeline for photo materials, and adding one is a dependency decision of its own. So CC0 sets are
mostly references today, and their value is in exact, reusable proportions and palettes.

| Set | License | What fits our style | Use |
|---|---|---|---|
| Kenney (city, suburban, retro-urban, car and road kits) | CC0 | Flat-colored low-poly buildings, storefronts, awnings, AC units, fences, roof props: the closest match to our look | Fit: proportions and silhouettes for props we generate (rooftop units, dumpsters, fire hydrants, phone booths). Ship a mesh only if a later ADR adds a loader |
| Quaternius (low-poly packs) | CC0 | Low-poly props, vegetation, vehicles at a similar density | Same as Kenney |
| ambientCG (PBR materials) | CC0 | Brick, stucco, concrete, asphalt, roofing photos | Fit: average and variance colors of brick, stucco and tar roof per era palette; mortar-course and siding spacing. Ship a texture only if the renderer gains detail maps |
| Poly Haven (HDRIs, textures, models) | CC0 | HDRI skies at golden hour and overcast | Fit: calibrate the Preetham sky and exposure against a real sky; check our brick and stucco albedos |
| OpenGameArt, filtered to CC0 | Per asset | Rarely the right style | Check each file's license; skip CC BY-SA and GPL art |

Fonts: none. Lettering stays our own block font and the stroke shapes in `BUILDINGS.md`.

### How it feeds the place compiler (M6)
`placec --bbox ... --era 1985` reads only Ship-mode sources: 3DEP and NOAA terrain and coast, NHD
water, TIGER roads (filtered by the earliest vintage for old eras), NLCD/GIRAS land cover for
zones, FEMA or a PD/CC0 city layer for footprints and heights, a city's `year_built` for the era
layer. Style tags come from heuristics over footprint shape, use, height and year
(`Dingbat` = two-storey residential, 1955–1970, lot 15 m; `StripMall` = commercial, one storey,
depth < 25 m, set back ≥ 20 m from the arterial). The output Place is public domain data plus our
code's output, so it may be shipped in the repo or the binary, with the provenance header listing
each source. The M6 milestone text (`docs/MILESTONES.md`) changes from OSM/Overture to these.

### How it feeds the procedural facade grammar
Fit-mode numbers go into RON grammar files per era and zone (`docs/backlog/BUILDINGS.md`): bay
widths, floor heights, sign-band heights, awning depths, parapet and cornice profiles, rooftop
clutter counts, lot coverage and parking ratios. Each number has a comment naming the source and
item. The FEMA and city datasets give the distributions (frontage, depth, storeys by zone); HABS
drawings and the photo collections give the facade rules; CC0 kits and materials give prop
silhouettes and palettes.

## Alternatives
- **OSM with the compiled Place shipped as a separate ODbL file** (the old sketch). Legal, but it
  makes every real place a share-alike database and every agent a license reviewer; the owner
  ruled ODbL out.
- **Overture Maps.** Its OSM-derived themes are ODbL; the CDLA-Permissive `places` theme is
  today's businesses, useless for 1977 and full of real names.
- **Photogrammetry or textures from street-level imagery.** Copyrighted and against the services'
  terms, and the opposite of our art direction.
- **Buying a commercial building dataset.** Licenses forbid redistribution; it would bind the
  project to a vendor.

## Consequences
- Real places are US-only for now (federal public-domain data); elsewhere needs a new review.
- Compiled places have today's minus newer: streets from the earliest TIGER, buildings filtered by
  `year_built`. Lanes, facades and props come from the shared detailing pass. This is the honest
  approximation `docs/PLACES.md` already promises.
- A `CREDITS.md` at the repo root lists every Ship and Fit source. A PR that adds a fitted number
  without a source comment is sent back.
- Coverage gaps (no footprints for a city, no `year_built`) fall back to procedural parcels and
  buildings on the real road graph and terrain.
- `placec` deps stay license-clean and small: TIGER is shapefiles, 3DEP is GeoTIFF, NHD and FEMA
  are GeoPackage/FileGDB; prefer GeoJSON/shapefile exports and small readers over GDAL bindings.
  A reader over ~5k lines needs its own ADR.
