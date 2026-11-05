//! Procedurally drawn application icons (no binary assets needed): a C with a play button pointing
//! out through its opening, for the app on a warm tile and for the server (`correntd`) on a dark
//! one. `corrent --write-icons assets/icon` renders what packaging and the web client use.

/// Which of the two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    App,
    Server,
}

type Rgb = [f32; 3];

/// Corner radius of the tile, in the icon's 100 units.
const TILE_RADIUS: f32 = 22.5;
/// The C: radius of its centre line, and where it starts and ends, from the right, in degrees.
const RING_RADIUS: f32 = 30.0;
const RING_GAP: f32 = 42.0;
/// The play button: its corners before rounding, and how much rounding.
const PLAY: [(f32, f32); 3] = [(42.5, 38.5), (61.5, 50.0), (42.5, 61.5)];
const PLAY_ROUND: f32 = 2.5;

const fn rgb(hex: u32) -> Rgb {
    [((hex >> 16) & 255) as f32, ((hex >> 8) & 255) as f32, (hex & 255) as f32]
}

impl Icon {
    /// The tile's gradient, top left to bottom right.
    fn tile(self) -> (Rgb, Rgb) {
        match self {
            Icon::App => (rgb(0xFF6A55), rgb(0xE0247A)),
            Icon::Server => (rgb(0x1C1B33), rgb(0x0B0B18)),
        }
    }

    /// The mark's gradient, over the part of the tile it covers.
    fn mark(self) -> (Rgb, Rgb) {
        match self {
            Icon::App => (rgb(0xFFFFFF), rgb(0xFFFFFF)),
            Icon::Server => (rgb(0xFF8A4C), rgb(0xF0348F)),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Icon::App => "corrent",
            Icon::Server => "correntd",
        }
    }
}

/// The C's thickness at `size` pixels: heavier when small, or it would fade out at 16 px.
fn ring_width(size: u32) -> f32 {
    match size {
        0..=16 => 14.0,
        17..=24 => 12.0,
        25..=32 => 11.0,
        _ => 10.0,
    }
}

/// Distance from a point to the tile's edge (negative inside).
fn tile_distance(x: f32, y: f32) -> f32 {
    let half = 50.0 - TILE_RADIUS;
    let (dx, dy) = ((x - 50.0).abs() - half, (y - 50.0).abs() - half);
    dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - TILE_RADIUS
}

/// Distance to the C, with round ends.
fn ring_distance(x: f32, y: f32, width: f32) -> f32 {
    let (px, py) = (x - 50.0, y - 50.0);
    let angle = py.atan2(px).to_degrees().abs();
    let line = if angle >= RING_GAP {
        (px.hypot(py) - RING_RADIUS).abs()
    } else {
        let (s, c) = RING_GAP.to_radians().sin_cos();
        (px - RING_RADIUS * c).hypot(py.abs() - RING_RADIUS * s)
    };
    line - width / 2.0
}

/// Distance to the play button, with round corners.
fn play_distance(x: f32, y: f32) -> f32 {
    let mut nearest = f32::MAX;
    let mut inside = true;
    for i in 0..PLAY.len() {
        let (Some(&(ax, ay)), Some(&(bx, by))) = (PLAY.get(i), PLAY.get((i + 1) % PLAY.len())) else { continue };
        let (ex, ey) = (bx - ax, by - ay);
        let (wx, wy) = (x - ax, y - ay);
        let t = ((wx * ex + wy * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        nearest = nearest.min((wx - ex * t).hypot(wy - ey * t));
        // Clockwise on screen: inside is on the same side of every edge.
        inside &= ex * wy - ey * wx >= 0.0;
    }
    (if inside { -nearest } else { nearest }) - PLAY_ROUND
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// RGBA pixels of an icon at `size`×`size`.
#[expect(clippy::indexing_slicing, reason = "fixed-size colour arrays and a buffer sized for every pixel")]
pub fn render(icon: Icon, size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let unit = 100.0 / size as f32;
    let width = ring_width(size);
    let ((t0, t1), (m0, m1)) = (icon.tile(), icon.mark());
    // A pixel's coverage from a distance in the icon's units.
    let cover = |d: f32| (0.5 - d / unit).clamp(0.0, 1.0);
    for y in 0..size {
        for x in 0..size {
            let (ux, uy) = ((x as f32 + 0.5) * unit, (y as f32 + 0.5) * unit);
            let alpha = cover(tile_distance(ux, uy));
            if alpha <= 0.0 {
                continue;
            }
            let along = (ux + uy) / 200.0;
            let mark = cover(ring_distance(ux, uy, width).min(play_distance(ux, uy)));
            // The mark's gradient runs across the mark alone, not the whole tile.
            let colour = mix(mix(t0, t1, along), mix(m0, m1, (along - 0.2) / 0.6), mark);
            let i = ((y * size + x) * 4) as usize;
            for c in 0..3 {
                px[i + c] = colour[c].round() as u8;
            }
            px[i + 3] = (alpha * 255.0).round() as u8;
        }
    }
    px
}

/// RGBA pixels of the app's icon at `size`×`size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    render(Icon::App, size)
}

pub fn app_icon() -> egui::IconData {
    let size = 128;
    egui::IconData { rgba: icon_rgba(size), width: size, height: size }
}

/// A frame of an .ico as an uncompressed bitmap: bottom-up BGRA, then an empty 1-bit mask, with the
/// height doubled to count the mask. What the older parts of Windows expect below 256 pixels:
/// PNG frames there come out as noise in GDI+.
fn ico_bitmap(rgba: &[u8], size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + rgba.len());
    for v in [40u32, size, size * 2, 0x0020_0001, 0, 0, 0, 0, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for row in rgba.chunks_exact(size as usize * 4).rev() {
        for &[r, g, b, a] in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    let mask_row = (size as usize).div_ceil(32) * 4;
    out.resize(out.len() + mask_row * size as usize, 0);
    out
}

/// A Windows .ico at the given sizes: bitmaps, and PNG at 256 where bitmaps would be large.
fn ico_of(icon: Icon, sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| {
            let rgba = render(icon, s);
            if s >= 256 { crate::util::encode_png(&rgba, s, s) } else { ico_bitmap(&rgba, s) }
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// The same drawing as an SVG, for what scales (the website, Linux icon themes).
fn svg(icon: Icon) -> String {
    let hex = |c: Rgb| format!("#{:02X}{:02X}{:02X}", c[0] as u8, c[1] as u8, c[2] as u8);
    let ((t0, t1), (m0, m1)) = (icon.tile(), icon.mark());
    let (s, c) = RING_GAP.to_radians().sin_cos();
    let (rx, ry) = (50.0 + RING_RADIUS * c, RING_RADIUS * s);
    let [(ax, ay), (bx, by), (cx, cy)] = PLAY;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{tile0}"/><stop offset="1" stop-color="{tile1}"/></linearGradient>
    <linearGradient id="mark" gradientUnits="userSpaceOnUse" x1="20" y1="20" x2="80" y2="80"><stop offset="0" stop-color="{mark0}"/><stop offset="1" stop-color="{mark1}"/></linearGradient>
  </defs>
  <rect width="100" height="100" rx="{TILE_RADIUS}" fill="url(#tile)"/>
  <path d="M{rx:.3} {y0:.3}A{RING_RADIUS} {RING_RADIUS} 0 1 0 {rx:.3} {y1:.3}" fill="none" stroke="url(#mark)" stroke-width="10" stroke-linecap="round"/>
  <path d="M{ax} {ay}L{bx} {by}L{cx} {cy}Z" fill="url(#mark)" stroke="url(#mark)" stroke-width="{round}" stroke-linejoin="round"/>
</svg>
"##,
        tile0 = hex(t0),
        tile1 = hex(t1),
        mark0 = hex(m0),
        mark1 = hex(m1),
        y0 = 50.0 - ry,
        y1 = 50.0 + ry,
        round = PLAY_ROUND * 2.0,
    )
}

/// Development helper: `corrent --write-icons <dir>` renders the icon sets used for packaging.
pub fn write_icon_set(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for icon in [Icon::App, Icon::Server] {
        let name = icon.name();
        for s in [32u32, 64, 128, 256, 512] {
            std::fs::write(dir.join(format!("{name}-{s}.png")), crate::util::encode_png(&render(icon, s), s, s))?;
        }
        std::fs::write(dir.join(format!("{name}.ico")), ico_of(icon, &[16, 24, 32, 48, 64, 128, 256]))?;
        std::fs::write(dir.join(format!("{name}.svg")), svg(icon))?;
    }
    // For web pages: small, unlike the executables' full set.
    std::fs::write(dir.join("favicon.ico"), ico_of(Icon::App, &[16, 32, 48]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &[u8], size: u32, x: f32, y: f32) -> [u8; 4] {
        let (px, py) = ((x * size as f32 / 100.0) as u32, (y * size as f32 / 100.0) as u32);
        let i = ((py * size + px) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn the_mark_is_where_it_should_be() {
        let img = render(Icon::App, 256);
        assert_eq!(pixel(&img, 256, 0.5, 0.5)[3], 0, "the corner is rounded off");
        assert_eq!(pixel(&img, 256, 20.0, 50.0), [255, 255, 255, 255], "the C's left side");
        assert_eq!(pixel(&img, 256, 50.0, 50.0), [255, 255, 255, 255], "the play button");
        assert_ne!(pixel(&img, 256, 83.0, 50.0)[..3], [255, 255, 255], "the C's opening");
        assert_ne!(pixel(&img, 256, 37.0, 50.0)[..3], [255, 255, 255], "between the C and the button");
    }

    #[test]
    fn the_play_button_points_right_through_the_opening() {
        // Past the C's ends, along the middle, nothing but the button's tip.
        assert!(play_distance(61.0, 50.0) < 0.0);
        assert!(play_distance(61.0, 40.0) > 0.0);
        assert!(ring_distance(61.0, 50.0, 10.0) > 0.0, "the opening is clear");
    }

    #[test]
    fn the_server_has_the_same_mark_on_a_dark_tile() {
        let (app, server) = (render(Icon::App, 128), render(Icon::Server, 128));
        let dark = pixel(&server, 128, 37.0, 50.0);
        assert!(dark[0] < 60 && dark[1] < 60 && dark[2] < 80, "{dark:?}");
        for (x, y) in [(20.0, 50.0), (50.0, 50.0)] {
            let mark = pixel(&server, 128, x, y);
            assert!(mark[0] > 200 && mark[1] > 40, "warm at {x},{y}: {mark:?}");
            assert_eq!(pixel(&app, 128, x, y)[3], mark[3]);
        }
    }

    #[test]
    fn small_sizes_keep_the_opening_and_a_heavier_c() {
        assert!(ring_width(16) > ring_width(256));
        let img = render(Icon::App, 16);
        assert_eq!(img.len(), 16 * 16 * 4);
        let white = |p: [u8; 4]| p[0] > 230 && p[1] > 230 && p[2] > 230;
        assert!(white(pixel(&img, 16, 20.0, 50.0)));
        assert!(!white(pixel(&img, 16, 92.0, 50.0)));
    }

    /// `assets/icon` is what packaging, the web client and the Windows executables use.
    #[test]
    fn the_checked_in_icons_are_what_this_draws() {
        for (icon, name) in [(Icon::App, "corrent"), (Icon::Server, "correntd")] {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icon");
            // Pixels, not bytes: the checked-in files are compressed further than `encode_png` does.
            let png = image::open(dir.join(format!("{name}-64.png"))).unwrap().into_rgba8();
            assert!(png.as_raw() == &render(icon, 64), "run corrent --write-icons assets/icon");
            let vector = std::fs::read_to_string(dir.join(format!("{name}.svg"))).unwrap();
            assert_eq!(vector.replace("\r\n", "\n"), svg(icon), "run corrent --write-icons assets/icon");
        }
    }

    #[test]
    fn ico_frames_are_bitmaps_below_256_and_stay_in_bounds() {
        let data = ico_of(Icon::App, &[16, 256]);
        let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize;
        assert_eq!(data[..6], [0, 0, 1, 0, 2, 0]);
        let (len, at) = (u32_at(6 + 8), u32_at(6 + 12));
        assert_eq!(len, 40 + 16 * 16 * 4 + 16 * 4, "header, pixels, mask");
        assert_eq!((u32_at(at), u32_at(at + 4), u32_at(at + 8)), (40, 16, 32));
        // Bottom-up: the first pixel stored is the bottom left corner, rounded off.
        assert_eq!(data[at + 40 + 3], 0);
        let (len2, at2) = (u32_at(6 + 16 + 8), u32_at(6 + 16 + 12));
        assert_eq!(data[at2..at2 + 4], [0x89, b'P', b'N', b'G']);
        assert_eq!(at2 + len2, data.len());
    }

    #[test]
    fn the_svg_uses_the_same_colours() {
        assert!(svg(Icon::App).contains("#FF6A55") && svg(Icon::App).contains("#FFFFFF"));
        assert!(svg(Icon::Server).contains("#1C1B33") && svg(Icon::Server).contains("#F0348F"));
    }
}
