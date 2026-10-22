//! `--preview <dir>`: software-rendered PNGs of the strip on light and dark
//! taskbars, using the same renderer and placement maths as the app.

use crate::display::{self, Rgb};
use crate::hidpp::Battery;
use crate::layout::{self, Rect};
use crate::render::{self, Image};

fn b(p: u8, charging: bool) -> Option<Battery> {
    Some(Battery {
        percent: p,
        approx: false,
        charging,
    })
}

pub fn run(dir: &str) -> Result<Vec<String>, String> {
    let font = render::load_font()?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let cases: [(Option<Battery>, Option<Battery>); 4] = [
        (b(85, false), None),
        (b(35, false), b(12, false)),
        (b(60, true), b(100, false)),
        (None, b(51, false)),
    ];
    let themes: [(&str, Rgb, Rgb); 2] = [
        ("light", [0xEE, 0xEE, 0xF2], [0xD6, 0xD6, 0xDC]),
        ("dark", [0x1F, 0x1F, 0x24], [0x33, 0x33, 0x3A]),
    ];
    let mut written = Vec::new();
    for (name, bg, tray_c) in themes {
        let dpis = [96u32, 144];
        let width = 560;
        let gap = 10;
        let rows: u32 = dpis
            .iter()
            .map(|d| layout::scale(48, *d) as u32 * cases.len() as u32 + gap * cases.len() as u32)
            .sum();
        let mut img = Image::new(width, rows + gap, [0x80, 0x80, 0x80]);
        let mut y = gap as i32;
        for dpi in dpis {
            let th = layout::scale(48, dpi);
            for (m, h) in cases.iter() {
                let tb = Rect::new(0, y, width as i32, y + th);
                img.fill(tb.l, tb.t, tb.w(), tb.h(), bg);
                let tray_w = layout::scale(180, dpi);
                let tray = Rect::new(tb.r - tray_w, tb.t, tb.r, tb.b);
                img.fill(tray.l + 4, tray.t + th / 5, tray.w() - 8, th - 2 * (th / 5), tray_c);
                let lines = display::lines(true, m.as_ref(), true, h.as_ref());
                let px = layout::font_px(th, lines.len());
                let outline = layout::outline_px(dpi);
                let c = render::render_strip(&font, &lines, px, th as u32, outline);
                let mon = Rect::new(0, 0, width as i32, 100_000);
                let p = layout::place(&tb, &mon, Some(&tray), c.w as i32, c.h as i32, layout::scale(6, dpi), (140, 0))
                    .ok_or("placement failed")?;
                img.blit(&c, p.rect.l, p.rect.t);
                y += th + gap as i32;
            }
        }
        let path = std::path::Path::new(dir).join(format!("preview-{}.png", name));
        std::fs::write(&path, img.png()).map_err(|e| e.to_string())?;
        written.push(path.display().to_string());
    }
    Ok(written)
}
