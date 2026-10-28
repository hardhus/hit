use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parses `rrggbb` or `#rrggbb`.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let channel = |range| u8::from_str_radix(hex.get(range)?, 16).ok();
        Some(Self(channel(0..2)?, channel(2..4)?, channel(4..6)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    pub color: Option<Rgb>,
    pub description: Option<String>,
}

impl Label {
    pub fn new(name: impl Into<String>, color: Option<Rgb>) -> Self {
        Self {
            name: name.into(),
            color,
            description: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(Rgb::from_hex("#ff2e97"), Some(Rgb(0xff, 0x2e, 0x97)));
        assert_eq!(Rgb::from_hex("00b4ff"), Some(Rgb(0x00, 0xb4, 0xff)));
        assert_eq!(Rgb::from_hex("fff"), None);
        assert_eq!(Rgb::from_hex("zzzzzz"), None);
        assert_eq!(Rgb::from_hex("ééé"), None);
    }
}
