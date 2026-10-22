//! Software renderer for the strip (used by the app and by --preview), and
//! a minimal dependency-free PNG writer.

use crate::display::{Line, Rgb};
use fontdue::{Font, FontSettings};

/// Premultiplied BGRA, top-down, 4 bytes per pixel (what
/// UpdateLayeredWindow expects).
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

pub const FONT_CANDIDATES: [&str; 6] = [
    r"C:\Windows\Fonts\seguisb.ttf",
    r"C:\Windows\Fonts\segoeuib.ttf",
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];

pub fn load_font() -> Result<Font, String> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let mut paths: Vec<String> = ["seguisb.ttf", "segoeuib.ttf", "segoeui.ttf", "arial.ttf"]
        .iter()
        .map(|f| format!(r"{}\Fonts\{}", windir, f))
        .collect();
    paths.extend(FONT_CANDIDATES.iter().map(|s| s.to_string()));
    for p in paths {
        if let Ok(bytes) = std::fs::read(&p) {
            if let Ok(f) = Font::from_bytes(bytes, FontSettings::default()) {
                return Ok(f);
            }
        }
    }
    Err("no usable font found".into())
}

fn line_width(font: &Font, text: &str, px: f32) -> f32 {
    let mut w = 0.0;
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if let Some(p) = prev {
            w += font.horizontal_kern(p, c, px).unwrap_or(0.0);
        }
        w += font.metrics(c, px).advance_width;
        prev = Some(c);
    }
    w
}

/// Render `lines` stacked and centred. `box_h` fixes the canvas height
/// (taskbar thickness); width is the text width plus padding.
pub fn render_strip(font: &Font, lines: &[Line], px: f32, box_h: u32, outline: u32) -> Canvas {
    let pad = outline as i32 + (px * 0.25).round() as i32;
    let widths: Vec<f32> = lines.iter().map(|l| line_width(font, &l.text, px)).collect();
    let text_w = widths.iter().copied().fold(0.0f32, f32::max).ceil() as i32;
    let w = (text_w + 2 * pad).max(1) as u32;
    let h = box_h.max(1);
    let (ascent, descent) = match font.horizontal_line_metrics(px) {
        Some(m) => (m.ascent, m.descent),
        None => (px * 0.8, -px * 0.2),
    };
    // Line pitch tight like the taskbar clock.
    let line_h = (ascent - descent) * 1.02;
    let block_h = line_h * lines.len() as f32;
    let top = (h as f32 - block_h) / 2.0;

    let n = (w * h) as usize;
    let mut cov = vec![0f32; n];
    let mut col: Vec<Rgb> = vec![[0, 0, 0]; n];
    for (li, line) in lines.iter().enumerate() {
        let baseline = (top + line_h * li as f32 + ascent + (line_h - (ascent - descent)) / 2.0).round() as i32;
        let mut pen = pad as f32 + (text_w as f32 - widths[li]) / 2.0;
        let mut prev: Option<char> = None;
        for c in line.text.chars() {
            if let Some(p) = prev {
                pen += font.horizontal_kern(p, c, px).unwrap_or(0.0);
            }
            let (m, bmp) = font.rasterize(c, px);
            let gx = pen.round() as i32 + m.xmin;
            let gy = baseline - (m.height as i32 + m.ymin);
            for yy in 0..m.height as i32 {
                for xx in 0..m.width as i32 {
                    let (x, y) = (gx + xx, gy + yy);
                    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                        continue;
                    }
                    let a = bmp[(yy * m.width as i32 + xx) as usize] as f32 / 255.0;
                    let i = (y as u32 * w + x as u32) as usize;
                    if a > cov[i] {
                        cov[i] = a;
                    }
                    if a > 0.0 {
                        col[i] = line.colour;
                    }
                }
            }
            pen += m.advance_width;
            prev = Some(c);
        }
    }

    // Soft dark halo for readability on light and dark taskbars.
    let r = outline as i32;
    let reach = r + 1;
    let mut halo = vec![0f32; n];
    if r > 0 {
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let mut best = 0f32;
                for dy in -reach..=reach {
                    for dx in -reach..=reach {
                        let (sx, sy) = (x + dx, y + dy);
                        if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
                            continue;
                        }
                        let c = cov[(sy as u32 * w + sx as u32) as usize];
                        if c <= best {
                            continue;
                        }
                        let d = ((dx * dx + dy * dy) as f32).sqrt();
                        let wgt = if d <= r as f32 {
                            1.0
                        } else {
                            ((r as f32 + 0.75 - d) / 0.75).max(0.0)
                        };
                        let v = c * wgt;
                        if v > best {
                            best = v;
                        }
                    }
                }
                halo[(y as u32 * w + x as u32) as usize] = best;
            }
        }
    }
    const HALO_ALPHA: f32 = 0.42;
    let mut px_out = vec![0u8; n * 4];
    for i in 0..n {
        let a = cov[i];
        let ha = halo[i] * HALO_ALPHA;
        let out_a = a + ha * (1.0 - a);
        let c = col[i];
        // premultiplied: text colour * a over black halo
        px_out[i * 4] = (c[2] as f32 * a).round() as u8;
        px_out[i * 4 + 1] = (c[1] as f32 * a).round() as u8;
        px_out[i * 4 + 2] = (c[0] as f32 * a).round() as u8;
        px_out[i * 4 + 3] = (out_a * 255.0).round().min(255.0) as u8;
    }
    Canvas { w, h, px: px_out }
}

// ---------------------------------------------------------------- PNG

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut c = Vec::with_capacity(4 + data.len());
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    out.extend_from_slice(&c);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

/// Encode straight (non-premultiplied) RGBA as PNG, stored deflate blocks.
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w * 4 + 1) as usize * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    if raw.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        let len = c.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Opaque RGB image used for previews.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgb: Vec<u8>,
}

impl Image {
    pub fn new(w: u32, h: u32, bg: Rgb) -> Self {
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for _ in 0..w * h {
            rgb.extend_from_slice(&bg);
        }
        Image { w, h, rgb }
    }
    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
        for yy in y.max(0)..(y + h).min(self.h as i32) {
            for xx in x.max(0)..(x + w).min(self.w as i32) {
                let i = ((yy as u32 * self.w + xx as u32) * 3) as usize;
                self.rgb[i..i + 3].copy_from_slice(&c);
            }
        }
    }
    /// Composite a premultiplied BGRA canvas at (x, y).
    pub fn blit(&mut self, c: &Canvas, x: i32, y: i32) {
        for cy in 0..c.h as i32 {
            for cx in 0..c.w as i32 {
                let (ix, iy) = (x + cx, y + cy);
                if ix < 0 || iy < 0 || ix >= self.w as i32 || iy >= self.h as i32 {
                    continue;
                }
                let s = ((cy as u32 * c.w + cx as u32) * 4) as usize;
                let d = ((iy as u32 * self.w + ix as u32) * 3) as usize;
                let a = c.px[s + 3] as u32;
                for (k, sk) in [(0usize, 2usize), (1, 1), (2, 0)] {
                    let v = c.px[s + sk] as u32 + self.rgb[d + k] as u32 * (255 - a) / 255;
                    self.rgb[d + k] = v.min(255) as u8;
                }
            }
        }
    }
    pub fn png(&self) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((self.w * self.h * 4) as usize);
        for p in self.rgb.chunks(3) {
            rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        encode_png(self.w, self.h, &rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_adler_known_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn png_structure() {
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn blit_composites_premultiplied() {
        let mut img = Image::new(1, 1, [100, 100, 100]);
        // 50% black (premultiplied) over grey
        let c = Canvas {
            w: 1,
            h: 1,
            px: vec![0, 0, 0, 128],
        };
        img.blit(&c, 0, 0);
        assert_eq!(img.rgb, vec![49, 49, 49]);
        let c = Canvas {
            w: 1,
            h: 1,
            px: vec![10, 20, 30, 255],
        };
        img.blit(&c, 0, 0);
        assert_eq!(img.rgb, vec![30, 20, 10]);
    }

    #[test]
    fn renders_text_when_font_available() {
        let Ok(font) = load_font() else { return };
        let lines = crate::display::lines(true, None, true, None);
        let c = render_strip(&font, &lines, 14.4, 48, 1);
        assert_eq!(c.h, 48);
        assert!(c.w > 20 && c.w < 200);
        assert!(c.px.chunks(4).any(|p| p[3] == 255));
        // premultiplied invariant
        assert!(c.px.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    }
}
