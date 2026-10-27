//! Front matter as a list of named values.

/// Front matter made only of `name: value` lines.
///
/// That is the shape apps write for titles, dates and tags. Anything richer
/// (nested maps, block lists, multi-line strings) has no [`Properties`] and
/// is edited as text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Properties(Vec<(String, String)>);

impl Properties {
    /// The properties in `yaml`, if it is that simple.
    #[must_use]
    pub fn parse(yaml: &str) -> Option<Self> {
        yaml.lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .map(|line| {
                if line.starts_with(char::is_whitespace) || line.starts_with('-') {
                    return None;
                }
                let (name, value) = line.split_once(':')?;
                let (name, value) = (name.trim(), value.trim());
                if name.is_empty() || matches!(value, "|" | ">" | "|-" | ">-") {
                    return None;
                }
                Some((name.to_owned(), unquote(value).to_owned()))
            })
            .collect::<Option<Vec<_>>>()
            .map(Self)
    }

    #[must_use]
    pub fn entries(&self) -> &[(String, String)] {
        &self.0
    }

    /// Set `name`, adding it at the end when new. An empty name is ignored.
    pub fn set(&mut self, name: &str, value: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let value = value.trim().to_owned();
        match self.0.iter_mut().find(|(current, _)| current == name) {
            Some((_, current)) => *current = value,
            None => self.0.push((name.to_owned(), value)),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(current, _)| current != name);
    }

    /// YAML for these properties, quoting values YAML would misread.
    #[must_use]
    pub fn to_yaml(&self) -> String {
        self.0
            .iter()
            .map(|(name, value)| format!("{name}: {}", quote(value)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn unquote(value: &str) -> &str {
    ['"', '\'']
        .into_iter()
        .find_map(|quote| {
            value
                .strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))
        })
        .unwrap_or(value)
}

fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && !value.contains(": ")
        && !value.contains(" #")
        && !value.starts_with(|ch: char| "&*!|>%@`\"'{".contains(ch))
        && value.trim() == value;
    if plain {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::Properties;

    #[test]
    fn flat_front_matter_round_trips() {
        let mut properties =
            Properties::parse("title: Sample\ntags: [a, b]\nnote: \"x: y\"\n").expect("flat");
        assert_eq!(properties.entries()[2].1, "x: y");
        properties.set("author", "Ada");
        properties.remove("tags");
        assert_eq!(
            properties.to_yaml(),
            "title: Sample\nnote: \"x: y\"\nauthor: Ada"
        );
    }

    #[test]
    fn nested_yaml_is_not_a_property_list() {
        assert!(Properties::parse("tags:\n  - a\n  - b").is_none());
        assert!(Properties::parse("body: |\n  text").is_none());
    }
}
