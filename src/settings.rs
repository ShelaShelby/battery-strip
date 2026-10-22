//! Small key=value settings file in %APPDATA%\battery-strip.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub show_mouse: bool,
    pub show_headset: bool,
    /// Monitor device names (e.g. \\.\DISPLAY2) where the strip is hidden.
    pub hidden_monitors: Vec<String>,
    /// Fallback placement when the tray rect is unknown: logical px from the
    /// right (or bottom) end of the taskbar, and upward shift.
    pub fallback_offset_x: i32,
    pub fallback_offset_y: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_mouse: true,
            show_headset: true,
            hidden_monitors: vec![],
            fallback_offset_x: 140,
            fallback_offset_y: 0,
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "show_mouse" => s.show_mouse = parse_bool(v).unwrap_or(s.show_mouse),
                "show_headset" => s.show_headset = parse_bool(v).unwrap_or(s.show_headset),
                "hidden_monitors" => s.hidden_monitors = v.split('|').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect(),
                "fallback_offset_x" => s.fallback_offset_x = v.parse::<i32>().map(|x| x.clamp(0, 4000)).unwrap_or(s.fallback_offset_x),
                "fallback_offset_y" => s.fallback_offset_y = v.parse::<i32>().map(|x| x.clamp(-500, 500)).unwrap_or(s.fallback_offset_y),
                _ => {}
            }
        }
        s
    }

    pub fn serialize(&self) -> String {
        format!(
            "# Battery Strip settings\nshow_mouse={}\nshow_headset={}\nhidden_monitors={}\n# fallback placement if the tray area cannot be read (logical px)\nfallback_offset_x={}\nfallback_offset_y={}\n",
            self.show_mouse as u8,
            self.show_headset as u8,
            self.hidden_monitors.join("|"),
            self.fallback_offset_x,
            self.fallback_offset_y
        )
    }

    pub fn monitor_enabled(&self, name: &str) -> bool {
        !self.hidden_monitors.iter().any(|m| m.eq_ignore_ascii_case(name))
    }

    pub fn toggle_monitor(&mut self, name: &str) {
        if self.monitor_enabled(name) {
            self.hidden_monitors.push(name.to_string());
        } else {
            self.hidden_monitors.retain(|m| !m.eq_ignore_ascii_case(name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let mut s = Settings {
            show_headset: false,
            fallback_offset_x: 200,
            ..Settings::default()
        };
        s.toggle_monitor(r"\\.\DISPLAY2");
        let t = s.serialize();
        assert_eq!(Settings::parse(&t), s);
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(
            Settings::parse("garbage\nshow_mouse=maybe\nfallback_offset_x=-5"),
            Settings {
                fallback_offset_x: 0,
                ..Settings::default()
            }
        );
    }

    #[test]
    fn monitor_toggle() {
        let mut s = Settings::default();
        assert!(s.monitor_enabled(r"\\.\DISPLAY1"));
        s.toggle_monitor(r"\\.\DISPLAY1");
        assert!(!s.monitor_enabled(r"\\.\display1"));
        s.toggle_monitor(r"\\.\DISPLAY1");
        assert!(s.monitor_enabled(r"\\.\DISPLAY1"));
    }
}
