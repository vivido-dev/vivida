//! UI locale selection: parsing language tags and detecting the system locale.

// Rust guideline compliant 2026-10-07

use crate::platform::preferred_language_tag;

/// A language the chrome UI can render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    /// English, the fallback for every unlisted language.
    #[default]
    En,
    /// Traditional Chinese.
    ZhHant,
    /// Simplified Chinese.
    ZhHans,
    /// Japanese.
    Ja,
}

impl Locale {
    /// Every locale, default first.
    pub const ALL: &[Self] = &[Self::En, Self::ZhHant, Self::ZhHans, Self::Ja];

    /// The locale the chrome should render in.
    ///
    /// `VIVIDA_LOCALE` overrides the system preference; an unparseable value falls through to
    /// system detection rather than forcing English.
    pub fn detect() -> Self {
        std::env::var("VIVIDA_LOCALE")
            .ok()
            .and_then(|tag| Self::from_tag(&tag))
            .or_else(|| preferred_language_tag().and_then(|tag| Self::from_tag(&tag)))
            .unwrap_or_default()
    }

    /// Parse a BCP-47-style or environment-style language tag.
    ///
    /// Matching is ASCII-case-insensitive and accepts `-` and `_` separators plus
    /// `zh_TW.UTF-8@pinyin`-style suffixes. Chinese splits by script subtag, then by region,
    /// and defaults to Simplified; everything unlisted returns `None` so the caller falls back
    /// to English.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let tag = tag.trim();
        let tag = tag.split_once('.').map_or(tag, |(tag, _)| tag);
        let tag = tag.split_once('@').map_or(tag, |(tag, _)| tag);
        let mut subtags = tag.split(['-', '_']).filter(|subtag| !subtag.is_empty());
        let language = subtags.next()?.to_ascii_lowercase();
        let mut script = None;
        let mut region = None;
        for subtag in subtags {
            // In a well-formed tag only the script subtag is four characters long.
            if subtag.len() == 4 {
                script.get_or_insert_with(|| subtag.to_ascii_lowercase());
            } else {
                region.get_or_insert_with(|| subtag.to_ascii_lowercase());
            }
        }
        match language.as_str() {
            "en" => Some(Self::En),
            "ja" => Some(Self::Ja),
            "zh" => match script.as_deref() {
                Some("hant") => Some(Self::ZhHant),
                Some("hans") => Some(Self::ZhHans),
                _ => match region.as_deref() {
                    Some("tw" | "hk" | "mo") => Some(Self::ZhHant),
                    // Bare `zh` and every unlisted region default to Simplified, the script
                    // with the most speakers.
                    _ => Some(Self::ZhHans),
                },
            },
            _ => None,
        }
    }
}

// Referenced so `ALL` — consumed by tests alone — is used by plain builds too.
const _: usize = Locale::ALL.len();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_parse_across_scripts_regions_and_env_suffixes() {
        assert_eq!(Locale::from_tag("en"), Some(Locale::En));
        assert_eq!(Locale::from_tag("en_US"), Some(Locale::En));
        assert_eq!(Locale::from_tag("EN-us"), Some(Locale::En));
        assert_eq!(Locale::from_tag("ja"), Some(Locale::Ja));
        assert_eq!(Locale::from_tag("ja_JP.UTF-8"), Some(Locale::Ja));
        assert_eq!(Locale::from_tag("zh"), Some(Locale::ZhHans));
        assert_eq!(Locale::from_tag("zh-CN"), Some(Locale::ZhHans));
        assert_eq!(Locale::from_tag("zh_SG"), Some(Locale::ZhHans));
        assert_eq!(Locale::from_tag("zh-Hans"), Some(Locale::ZhHans));
        assert_eq!(Locale::from_tag("zh_Hans_CN"), Some(Locale::ZhHans));
        assert_eq!(Locale::from_tag("zh-Hans-TW"), Some(Locale::ZhHans));
        assert_eq!(Locale::from_tag("zh-TW"), Some(Locale::ZhHant));
        assert_eq!(Locale::from_tag("zh_HK"), Some(Locale::ZhHant));
        assert_eq!(Locale::from_tag("zh-mo"), Some(Locale::ZhHant));
        assert_eq!(Locale::from_tag("zh-Hant-TW"), Some(Locale::ZhHant));
        assert_eq!(Locale::from_tag("zh-Hant-CN"), Some(Locale::ZhHant));
        assert_eq!(Locale::from_tag("zh_tw@pinyin"), Some(Locale::ZhHant));
    }

    #[test]
    fn unsupported_and_absent_tags_parse_to_none() {
        assert_eq!(Locale::from_tag(""), None);
        assert_eq!(Locale::from_tag("  "), None);
        assert_eq!(Locale::from_tag("C"), None);
        assert_eq!(Locale::from_tag("POSIX"), None);
        assert_eq!(Locale::from_tag("C.UTF-8"), None);
        assert_eq!(Locale::from_tag("fr"), None);
        assert_eq!(Locale::from_tag("ko-KR"), None);
    }
}
