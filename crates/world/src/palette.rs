//! Per-place color palettes.
//!
//! Colors are authored in sRGB (as a painter would pick them) and stored converted to
//! linear light, which is what vertex colors and the HDR renderer expect. The conversion
//! table is computed at compile time, so a palette is plain constant data.

/// Linear-light RGB in unorm8.
pub type Rgb = [u8; 3];

/// The 1980s Southern California beach-town look: sun-bleached pastels and stucco,
/// pale concrete, dusty lawns, and sodium and neon emissives for the night.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    // Walls.
    pub stucco_cream: Rgb,
    pub stucco_white: Rgb,
    pub salmon: Rgb,
    pub pastel_pink: Rgb,
    pub peach: Rgb,
    pub mint: Rgb,
    pub sky_blue: Rgb,
    pub lavender: Rgb,
    pub sand: Rgb,
    pub brick_red: Rgb,
    pub brick_brown: Rgb,
    // Trim and accents.
    pub teal: Rgb,
    pub terracotta: Rgb,
    pub navy: Rgb,
    // Ground and street.
    pub concrete: Rgb,
    pub concrete_dark: Rgb,
    pub asphalt: Rgb,
    pub asphalt_worn: Rgb,
    pub paint_white: Rgb,
    pub paint_yellow: Rgb,
    pub roof_gravel: Rgb,
    pub lawn: Rgb,
    pub lawn_dry: Rgb,
    // Plants.
    pub palm_green: Rgb,
    pub leaf_green: Rgb,
    pub leaf_light: Rgb,
    pub bark: Rgb,
    // Materials.
    pub glass: Rgb,
    pub metal_dark: Rgb,
    pub metal_light: Rgb,
    pub hydrant_red: Rgb,
    // Emissive.
    pub sodium: Rgb,
    pub window_warm: Rgb,
    pub neon_pink: Rgb,
    pub neon_cyan: Rgb,
    pub neon_red: Rgb,
    pub neon_green: Rgb,
}

impl Palette {
    pub const fn socal_1985() -> Palette {
        Palette {
            stucco_cream: srgb(0xF0E2C4),
            stucco_white: srgb(0xEDEAE0),
            salmon: srgb(0xEE9A80),
            pastel_pink: srgb(0xF2B9BC),
            peach: srgb(0xF4C79A),
            mint: srgb(0xADDCC2),
            sky_blue: srgb(0x9CC9E0),
            lavender: srgb(0xBFADD6),
            sand: srgb(0xDCC59C),
            brick_red: srgb(0x9A4434),
            brick_brown: srgb(0x7C4E3A),
            teal: srgb(0x23989C),
            terracotta: srgb(0xBE5E40),
            navy: srgb(0x2C3E66),
            concrete: srgb(0xB9B4AA),
            concrete_dark: srgb(0x8C8A84),
            asphalt: srgb(0x48484A),
            asphalt_worn: srgb(0x5E5D5A),
            paint_white: srgb(0xE8E8E0),
            paint_yellow: srgb(0xE6BA30),
            roof_gravel: srgb(0x7E7A72),
            lawn: srgb(0x6A9440),
            lawn_dry: srgb(0x9C9A58),
            palm_green: srgb(0x4A7A34),
            leaf_green: srgb(0x3E6A2C),
            leaf_light: srgb(0x6E9A3E),
            bark: srgb(0x6A503A),
            glass: srgb(0x32485A),
            metal_dark: srgb(0x3A3C40),
            metal_light: srgb(0x9A9EA4),
            hydrant_red: srgb(0xC42820),
            sodium: srgb(0xFFAA50),
            window_warm: srgb(0xFFD49A),
            neon_pink: srgb(0xFF3CAA),
            neon_cyan: srgb(0x3CE6FF),
            neon_red: srgb(0xFF3020),
            neon_green: srgb(0x60FF70),
        }
    }

    /// Exterior wall colors for stucco buildings.
    pub fn stucco_walls(&self) -> [Rgb; 9] {
        [
            self.stucco_cream,
            self.stucco_white,
            self.salmon,
            self.pastel_pink,
            self.peach,
            self.mint,
            self.sky_blue,
            self.lavender,
            self.sand,
        ]
    }

    /// Awning and canopy accent colors.
    pub fn awnings(&self) -> [Rgb; 6] {
        [
            self.teal,
            self.salmon,
            self.terracotta,
            self.navy,
            self.sky_blue,
            self.pastel_pink,
        ]
    }

    /// Sign and neon colors.
    pub fn neon(&self) -> [Rgb; 5] {
        [
            self.neon_pink,
            self.neon_cyan,
            self.neon_red,
            self.neon_green,
            self.window_warm,
        ]
    }

    /// Body colors for parked cars.
    pub fn cars(&self) -> [Rgb; 8] {
        [
            self.stucco_white,
            self.brick_red,
            self.navy,
            self.sand,
            self.sky_blue,
            self.metal_dark,
            self.metal_light,
            self.teal,
        ]
    }
}

/// `0xRRGGBB` in sRGB to linear unorm8.
const fn srgb(hex: u32) -> Rgb {
    [
        SRGB_TO_LINEAR[((hex >> 16) & 0xFF) as usize],
        SRGB_TO_LINEAR[((hex >> 8) & 0xFF) as usize],
        SRGB_TO_LINEAR[(hex & 0xFF) as usize],
    ]
}

/// Scales a color, saturating; `s` in 1/256 steps (256 = unchanged).
pub fn shade(c: Rgb, s: u32) -> Rgb {
    let f = |v: u8| ((u32::from(v) * s) >> 8).min(255) as u8;
    [f(c[0]), f(c[1]), f(c[2])]
}

/// Linear blend of two colors; `t` in 1/256 steps.
pub fn mix(a: Rgb, b: Rgb, t: u32) -> Rgb {
    let t = t.min(256);
    let f = |x: u8, y: u8| ((u32::from(x) * (256 - t) + u32::from(y) * t) >> 8) as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

/// The sRGB transfer function inverted, rounded to unorm8.
pub const SRGB_TO_LINEAR: [u8; 256] = srgb_table();

const fn srgb_table() -> [u8; 256] {
    let mut out = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let c = i as f64 / 255.0;
        let lin = if c <= 0.04045 {
            c / 12.92
        } else {
            let x = (c + 0.055) / 1.055;
            // x^2.4 = x^2 * x^(2/5); the fifth root by Newton iteration.
            let x2 = x * x;
            let mut r = 1.0;
            let mut k = 0;
            while k < 40 {
                let r4 = r * r * r * r;
                r -= (r4 * r - x2) / (5.0 * r4);
                k += 1;
            }
            x2 * r
        };
        out[i] = (lin * 255.0 + 0.5) as u8;
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_table_matches_reference() {
        for (i, &got) in SRGB_TO_LINEAR.iter().enumerate() {
            let c = i as f64 / 255.0;
            let want = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
            let want = (want * 255.0 + 0.5) as u8;
            assert_eq!(got, want, "{i}");
        }
        assert_eq!(SRGB_TO_LINEAR[0], 0);
        assert_eq!(SRGB_TO_LINEAR[255], 255);
    }

    #[test]
    fn palette_is_constant_and_distinct() {
        let p = Palette::socal_1985();
        assert_eq!(p, Palette::socal_1985());
        assert_ne!(p.asphalt, p.concrete);
        assert_ne!(p.lawn, p.leaf_green);
        assert_eq!(shade([100, 200, 255], 256), [100, 200, 255]);
        assert_eq!(mix([0, 0, 0], [200, 100, 50], 128), [100, 50, 25]);
    }
}
