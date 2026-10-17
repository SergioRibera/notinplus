use std::fmt;
use std::str::FromStr;

/// Straight (non premultiplied) RGBA color used by inline styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// CSS hex notation, `#rrggbb` when opaque and `#rrggbbaa` otherwise.
    #[must_use]
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl From<(u8, u8, u8)> for Rgba {
    fn from((r, g, b): (u8, u8, u8)) -> Self {
        Self::rgb(r, g, b)
    }
}

impl From<(u8, u8, u8, u8)> for Rgba {
    fn from((r, g, b, a): (u8, u8, u8, u8)) -> Self {
        Self::new(r, g, b, a)
    }
}

/// Error returned when a CSS color value cannot be understood.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseColorError {
    pub input: String,
}

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported css color `{}`", self.input)
    }
}

impl std::error::Error for ParseColorError {}

impl FromStr for Rgba {
    type Err = ParseColorError;

    /// Accepts `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(..)`, `rgba(..)`
    /// and a handful of named colors.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let error = || ParseColorError {
            input: input.to_owned(),
        };
        let value = input.trim().to_ascii_lowercase();

        if let Some(hex) = value.strip_prefix('#') {
            return parse_hex(hex).ok_or_else(error);
        }
        if let Some(args) = value
            .strip_prefix("rgba(")
            .or_else(|| value.strip_prefix("rgb("))
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return parse_functional(args).ok_or_else(error);
        }
        named(&value).ok_or_else(error)
    }
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
    let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        3 | 4 => {
            let expand = |v: u8| v * 17;
            let a = if hex.len() == 4 {
                expand(digit(3)?)
            } else {
                255
            };
            Some(Rgba::new(
                expand(digit(0)?),
                expand(digit(1)?),
                expand(digit(2)?),
                a,
            ))
        }
        6 | 8 => {
            let a = if hex.len() == 8 { pair(6)? } else { 255 };
            Some(Rgba::new(pair(0)?, pair(2)?, pair(4)?, a))
        }
        _ => None,
    }
}

fn parse_functional(args: &str) -> Option<Rgba> {
    let parts: Vec<&str> = args
        .split([',', ' ', '/'])
        .filter(|part| !part.is_empty())
        .collect();
    let channel = |part: &str| part.parse::<u8>().ok();
    match parts.as_slice() {
        [r, g, b] => Some(Rgba::rgb(channel(r)?, channel(g)?, channel(b)?)),
        [r, g, b, a] => {
            let alpha = a.parse::<f32>().ok()?.clamp(0.0, 1.0);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "alpha is clamped to 0..=1 so the scaled value fits in u8"
            )]
            let alpha = (alpha * 255.0).round() as u8;
            Some(Rgba::new(channel(r)?, channel(g)?, channel(b)?, alpha))
        }
        _ => None,
    }
}

fn named(name: &str) -> Option<Rgba> {
    Some(match name {
        "black" => Rgba::rgb(0, 0, 0),
        "white" => Rgba::rgb(255, 255, 255),
        "red" => Rgba::rgb(255, 0, 0),
        "green" => Rgba::rgb(0, 128, 0),
        "blue" => Rgba::rgb(0, 0, 255),
        "yellow" => Rgba::rgb(255, 255, 0),
        "orange" => Rgba::rgb(255, 165, 0),
        "purple" => Rgba::rgb(128, 0, 128),
        "pink" => Rgba::rgb(255, 192, 203),
        "gray" | "grey" => Rgba::rgb(128, 128, 128),
        "transparent" => Rgba::new(0, 0, 0, 0),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_css_notations() {
        assert_eq!("#f00".parse(), Ok(Rgba::rgb(255, 0, 0)));
        assert_eq!("#00ff0080".parse(), Ok(Rgba::new(0, 255, 0, 128)));
        assert_eq!("rgb(1, 2, 3)".parse(), Ok(Rgba::rgb(1, 2, 3)));
        assert_eq!("rgba(1,2,3,0.5)".parse(), Ok(Rgba::new(1, 2, 3, 128)));
        assert_eq!("Red".parse(), Ok(Rgba::rgb(255, 0, 0)));
        assert!("nope".parse::<Rgba>().is_err());
    }

    #[test]
    fn hex_round_trip() {
        let color = Rgba::new(10, 20, 30, 40);
        assert_eq!(color.to_hex().parse(), Ok(color));
    }
}
