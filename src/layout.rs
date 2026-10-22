//! Taskbar placement maths (physical pixels, pure functions).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Rect {
    pub const fn new(l: i32, t: i32, r: i32, b: i32) -> Self {
        Rect { l, t, r, b }
    }
    pub fn w(&self) -> i32 {
        self.r - self.l
    }
    pub fn h(&self) -> i32 {
        self.b - self.t
    }
    pub fn is_empty(&self) -> bool {
        self.w() <= 0 || self.h() <= 0
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect::new(self.l.max(o.l), self.t.max(o.t), self.r.min(o.r), self.b.min(o.b))
    }
    pub fn contains(&self, o: &Rect) -> bool {
        o.l >= self.l && o.t >= self.t && o.r <= self.r && o.b <= self.b
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

impl Edge {
    pub fn horizontal(self) -> bool {
        matches!(self, Edge::Bottom | Edge::Top)
    }
}

/// Which monitor edge the (visible part of the) taskbar sits on.
pub fn edge(tb: &Rect, mon: &Rect) -> Edge {
    if tb.w() >= tb.h() {
        if (tb.t - mon.t) > (mon.b - tb.b) {
            Edge::Bottom
        } else {
            Edge::Top
        }
    } else if (tb.l - mon.l) > (mon.r - tb.r) {
        Edge::Right
    } else {
        Edge::Left
    }
}

/// Visible taskbar area, or None when (auto-)hidden / off this monitor.
pub fn visible_taskbar(tb: &Rect, mon: &Rect) -> Option<Rect> {
    let v = tb.intersect(mon);
    if v.w() < 8 || v.h() < 8 {
        None
    } else {
        Some(v)
    }
}

/// Font pixel size for the strip, from the taskbar thickness.
pub fn font_px(thickness: i32, lines: usize) -> f32 {
    let f = match lines {
        0 | 1 => thickness as f32 * 0.36,
        _ => thickness as f32 * 0.30,
    };
    f.clamp(8.0, 64.0)
}

pub struct Placement {
    pub rect: Rect,
    pub used_tray: bool,
}

/// Place a strip of `w` x `h` on the taskbar, just before the notification
/// area (`tray`) when it is known and plausible, otherwise at
/// `fallback_offset` (physical px from the far end of the taskbar).
/// `margin` is the gap between strip and tray (physical px).
pub fn place(tb: &Rect, mon: &Rect, tray: Option<&Rect>, w: i32, h: i32, margin: i32, fallback_offset: (i32, i32)) -> Option<Placement> {
    let vis = visible_taskbar(tb, mon)?;
    let e = edge(&vis, mon);
    let tray = tray.filter(|t| !t.is_empty() && overlaps_enough(&vis, t));
    if e.horizontal() {
        let h = h.min(vis.h());
        let w = w.min(vis.w());
        let mut y = vis.t + (vis.h() - h) / 2;
        let (x, used) = match tray {
            Some(t) => (t.l - margin - w, true),
            None => {
                y -= fallback_offset.1;
                (vis.r - fallback_offset.0 - w, false)
            }
        };
        let x = x.max(vis.l).min(vis.r - w);
        y = y.max(vis.t).min(vis.b - h);
        Some(Placement {
            rect: Rect::new(x, y, x + w, y + h),
            used_tray: used,
        })
    } else {
        let w = w.min(vis.w());
        let h = h.min(vis.h());
        let mut x = vis.l + (vis.w() - w) / 2;
        let (y, used) = match tray {
            Some(t) => (t.t - margin - h, true),
            None => {
                x -= fallback_offset.1;
                (vis.b - fallback_offset.0 - h, false)
            }
        };
        let y = y.max(vis.t).min(vis.b - h);
        x = x.max(vis.l).min(vis.r - w);
        Some(Placement {
            rect: Rect::new(x, y, x + w, y + h),
            used_tray: used,
        })
    }
}

fn overlaps_enough(vis: &Rect, t: &Rect) -> bool {
    let i = vis.intersect(t);
    !i.is_empty() && i.w() * 2 >= t.w() && i.h() * 2 >= t.h()
}

/// Halo/outline radius in physical px for a dpi (1 px up to 191 dpi).
pub fn outline_px(dpi: u32) -> u32 {
    1 + dpi.saturating_sub(96) / 96
}

/// Scale a logical (96 dpi) length to physical pixels for `dpi`.
pub fn scale(v: i32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { 96 } else { dpi as i64 };
    ((v as i64 * dpi + 48) / 96) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect::new(0, 0, 1920, 1080);
    const TB: Rect = Rect::new(0, 1032, 1920, 1080);

    #[test]
    fn edges() {
        assert_eq!(edge(&TB, &MON), Edge::Bottom);
        assert_eq!(edge(&Rect::new(0, 0, 1920, 48), &MON), Edge::Top);
        assert_eq!(edge(&Rect::new(0, 0, 60, 1080), &MON), Edge::Left);
        assert_eq!(edge(&Rect::new(1860, 0, 1920, 1080), &MON), Edge::Right);
    }

    #[test]
    fn places_left_of_tray() {
        let tray = Rect::new(1700, 1032, 1920, 1080);
        let p = place(&TB, &MON, Some(&tray), 60, 48, 8, (140, 0)).unwrap();
        assert!(p.used_tray);
        assert_eq!(p.rect, Rect::new(1632, 1032, 1692, 1080));
        // shorter strip is centred vertically
        let p = place(&TB, &MON, Some(&tray), 60, 40, 8, (140, 0)).unwrap();
        assert_eq!(p.rect, Rect::new(1632, 1036, 1692, 1076));
    }

    #[test]
    fn fallback_when_no_tray_or_bogus_tray() {
        let p = place(&TB, &MON, None, 60, 48, 8, (140, 0)).unwrap();
        assert!(!p.used_tray);
        assert_eq!(p.rect, Rect::new(1720, 1032, 1780, 1080));
        // tray rect on another monitor -> ignored
        let bogus = Rect::new(3000, 1032, 3200, 1080);
        let p = place(&TB, &MON, Some(&bogus), 60, 48, 8, (140, 0)).unwrap();
        assert!(!p.used_tray);
        // empty tray rect -> ignored
        let p = place(&TB, &MON, Some(&Rect::default()), 60, 48, 8, (140, 0)).unwrap();
        assert!(!p.used_tray);
        // y offset moves the strip up but it stays on the taskbar
        let p = place(&TB, &MON, None, 60, 40, 8, (140, 100)).unwrap();
        assert_eq!(p.rect.t, 1032);
    }

    #[test]
    fn secondary_monitor_negative_coords() {
        let mon = Rect::new(-2560, -360, 0, 1080);
        let tb = Rect::new(-2560, 1008, 0, 1080);
        let p = place(&tb, &mon, None, 90, 72, 12, (210, 0)).unwrap();
        assert_eq!(p.rect, Rect::new(-300, 1008, -210, 1080));
    }

    #[test]
    fn clamped_inside_taskbar() {
        let tray = Rect::new(10, 1032, 200, 1080);
        let p = place(&TB, &MON, Some(&tray), 60, 48, 8, (140, 0)).unwrap();
        assert_eq!(p.rect.l, 0);
        let p = place(&TB, &MON, None, 5000, 100, 8, (140, 0)).unwrap();
        assert_eq!(p.rect, TB);
    }

    #[test]
    fn hidden_taskbar_gives_none() {
        // auto-hidden: only 2 px visible
        let tb = Rect::new(0, 1078, 1920, 1126);
        assert!(place(&tb, &MON, None, 60, 48, 8, (140, 0)).is_none());
    }

    #[test]
    fn vertical_taskbar() {
        let tb = Rect::new(0, 0, 62, 1080);
        let tray = Rect::new(0, 900, 62, 1080);
        let p = place(&tb, &MON, Some(&tray), 50, 40, 8, (140, 0)).unwrap();
        assert_eq!(p.rect, Rect::new(6, 852, 56, 892));
    }

    #[test]
    fn dpi_scaling_and_font() {
        assert_eq!(scale(8, 96), 8);
        assert_eq!(scale(8, 144), 12);
        assert_eq!(scale(140, 120), 175);
        assert_eq!(scale(10, 0), 10);
        assert_eq!(outline_px(96), 1);
        assert_eq!(outline_px(144), 1);
        assert_eq!(outline_px(192), 2);
        assert!((font_px(48, 2) - 14.4).abs() < 0.01);
        assert!((font_px(72, 2) - 21.6).abs() < 0.01);
        assert_eq!(font_px(10, 2), 8.0);
    }
}
