//! What every setting is: the table behind the settings reference and the
//! settings list of the desktop application.

/// The kind of value a setting takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// A number with a fraction, such as a length.
    Number,
    /// A whole number of things, such as layers.
    Count,
    /// On or off (`true`, `false`).
    Flag,
    /// One of the names in [`SettingInfo::choices`].
    Choice,
    /// Free text, such as custom G-code.
    Text,
}

/// Description of one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingInfo {
    /// Name as used in settings files.
    pub key: &'static str,
    /// Group it belongs to, such as `Printer` or `Speeds`.
    pub group: &'static str,
    /// Kind of value.
    pub kind: ValueKind,
    /// The allowed values of a [`ValueKind::Choice`]; empty otherwise.
    pub choices: &'static [&'static str],
    /// What it does, from the documentation of the field of
    /// [`Settings`](crate::Settings).
    pub description: &'static str,
}

/// Every setting in the order of [`Settings`](crate::Settings), generated
/// at build time from its fields and their doc comments. Allowed ranges of
/// numbers come from [`Settings::limits`](crate::Settings::limits).
pub const SETTINGS_INFO: &[SettingInfo] = include!(concat!(env!("OUT_DIR"), "/settings_info.rs"));

/// The description of the setting `key`.
pub fn setting_info(key: &str) -> Option<&'static SettingInfo> {
    SETTINGS_INFO.iter().find(|info| info.key == key)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::Settings;

    #[test]
    fn every_setting_is_described() {
        let values = Settings::default().values();
        let keys: Vec<&str> = SETTINGS_INFO.iter().map(|info| info.key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            values.keys().map(String::as_str).collect::<Vec<_>>()
        );
        for info in SETTINGS_INFO {
            assert!(!info.description.is_empty(), "{}", info.key);
            assert!(!info.group.is_empty(), "{}", info.key);
            assert_eq!(info.kind == ValueKind::Choice, !info.choices.is_empty());
            // Every choice is accepted and the default is one of them.
            for choice in info.choices {
                Settings::default().set(info.key, choice).unwrap();
            }
            if info.kind == ValueKind::Choice {
                assert!(info.choices.contains(&values[info.key].as_str()));
            }
        }
        let numbers = Settings::default().limits();
        for (key, ..) in &numbers {
            let kind = setting_info(key).unwrap().kind;
            assert!(
                matches!(kind, ValueKind::Number | ValueKind::Count),
                "{key}"
            );
        }
    }

    #[test]
    fn the_table_knows_kinds_groups_and_choices() {
        let wall_order = setting_info("wall_order").unwrap();
        assert_eq!(wall_order.choices, ["inner_outer", "outer_inner"]);
        assert_eq!(wall_order.group, "Process");
        assert_eq!(setting_info("bed_size_x").unwrap().group, "Printer");
        assert_eq!(setting_info("wall_loops").unwrap().kind, ValueKind::Count);
        assert_eq!(setting_info("supports").unwrap().kind, ValueKind::Flag);
        assert_eq!(setting_info("end_gcode").unwrap().kind, ValueKind::Text);
        assert_eq!(
            setting_info("layer_height").unwrap().description,
            "Layer height."
        );
        assert!(setting_info("nope").is_none());
    }
}
