# ADR 0007: landmarks as RON kits

Status: accepted, 2026-10-28 (the landmark gallery card)

## Context
The feel cards ask for landmarks: things you would slow down to look at. The owner picks each
one's look by eye from a contact sheet of lettered variants, so a landmark wants three or four
variants that are cheap to make and cheap to change after a pick. The basin's set pieces
(`world::setpieces`) are Rust code, one builder function per kind; that suits a piece with
behaviour (a drag strip's tree, a crossing's gates) but makes a variant a code change, a new
`PropKind`, a codec tag and a hash payload each time.

## Decision
A landmark is a kit: a small RON file, `assets/landmarks/<id>.ron` (ADR 0002's parser, no new
dependency), of a few primitives in the lot's own frame: boxes, struts (square beams between two
points), cylinders, lathed profiles and lamps, each in a named paint (a palette color, so it dresses
like its world, or a fixed sRGB one; a surface pattern; an optional glow). Variants are lettered A, B,
C in file order; a variant may start `from` an earlier one and add parts or repaint, so a kitsch take on
a grounded piece is a few lines. One `PropKind::Landmark { kit, variant }` covers every kit; the
grammar carries the kits (`Grammar::landmarks`, in file-name order), and `world::landmarks` builds the
mesh, the collision boxes (parts that start within 2 m of the lot) and the lamps' lights. `Kit::validate`
refuses unknown paints, parts off the lot or over 60 m, bad sizes and variants over 8,000 triangles.

Rust stays for what a kit cannot say: behaviour, lettering from the lexicon, anything that reads the
place around it. A landmark that needs those is a set piece and may use a kit for its body.

## Alternatives
Rust per landmark like `world::setpieces` (every variant is code, review and a codec change; no cheap
side-by-side variants). Imported meshes (glTF): a loader dependency, an asset pipeline and a licence
question for every model, and they would not take the world's palette.

## Consequences
New variants are data-only edits that the gallery (`place=gallery`) and its contact sheets
(`tools/screenshot/shoot.mjs --gallery <id>`) show at once. A kit's index is its place in file-name
order, so a kit is added, never renamed or removed, once a place stores it. The kit frame and the
primitive set are the format; growing it (a sweep, a sign face) is a change to this ADR.
