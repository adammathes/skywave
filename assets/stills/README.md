# World page stills

One 320×200 still per scenario (`assets/scenarios/<id>.ron`), named `<id>.still`: a 256-color palette
(768 bytes, sRGB) then one palette index per texel, row-major from the top-left (64,768 bytes),
committed and compiled into the game (`crates/app/src/scenario.rs`). Each is a frame of the web build in headless Chromium
(`tools/screenshot/shoot.mjs`, WebGL2 on SwiftShader) at 1280×800, cropped to an 864×540 window clear
of the HUD (x 264, y 130) and scaled down to 320×200, then quantized to 256 colors and written raw:

```
node tools/screenshot/shoot.mjs --size 1280x800 --url-query "?scenario=<id>&cam=chase-far&still=1" --out /tmp/<id>.png
python3 -c "from PIL import Image; im = Image.open('/tmp/<id>.png').convert('RGB').crop((264, 130, 1128, 670)).resize((320, 200), Image.LANCZOS); q = im.quantize(256, dither=Image.Dither.FLOYDSTEINBERG); pal = q.getpalette()[:768]; open('assets/stills/<id>.still', 'wb').write(bytes(pal + [0] * (768 - len(pal))) + q.tobytes())"
```

`mirage_valley_2001` is a stub on the coast's place until its own template lands; its still is the coast at
its default time (22:30) and should be shot again then.
