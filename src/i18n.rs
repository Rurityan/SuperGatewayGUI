//! Application-owned text only. Never translate configuration values or network payloads.
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}
impl Language {
    pub fn index(self) -> usize {
        match self {
            Self::System => 0,
            Self::Chinese => 1,
            Self::English => 2,
        }
    }
    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Chinese,
            2 => Self::English,
            _ => Self::System,
        }
    }
    pub fn english_for(self, ui_language: u16) -> bool {
        match self {
            Self::System => ui_language & 0x3ff != 0x04,
            Self::Chinese => false,
            Self::English => true,
        }
    }
}
static ENGLISH: AtomicBool = AtomicBool::new(false);
macro_rules! format_text {
    ($($arg:tt)*) => { $crate::i18n::text(&format!($($arg)*)).into_owned() };
}
pub(crate) use format_text;
pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}
pub fn initialize(language: Language) -> bool {
    let en = language
        .english_for(unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() });
    ENGLISH.swap(en, Ordering::Relaxed) != en
}
pub fn catalog() -> &'static [(String, String)] {
    static CATALOG: OnceLock<Vec<(String, String)>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/locales/en.json"))
            .expect("embedded translations")
    })
}
// Match formatted messages without replacing words inside user values. Placeholders
// are delimited by the original template; unmatched text is left intact.
fn parts(template: &str) -> Vec<&str> {
    let mut rest = template;
    let mut result = Vec::new();
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        result.push(&rest[..start]);
        rest = &rest[start + end + 1..];
    }
    result.push(rest);
    result
}
fn capture<'a>(input: &'a str, pattern: &str) -> Option<Vec<&'a str>> {
    let p = parts(pattern);
    if p.len() < 2 || p.iter().all(|s| s.is_empty()) {
        return None;
    }
    let mut rest = input.strip_prefix(p[0])?;
    let mut values = Vec::new();
    for (i, delimiter) in p.iter().enumerate().skip(1) {
        if i == p.len() - 1 {
            values.push(rest.strip_suffix(delimiter)?);
            return Some(values);
        }
        // Adjacent values remain concatenated; assign the combined value to
        // the last placeholder rather than guessing its internal boundary.
        if delimiter.is_empty() {
            values.push("");
            continue;
        }
        let end = rest.find(delimiter)?;
        values.push(&rest[..end]);
        rest = &rest[end + delimiter.len()..];
    }
    None
}
fn translate(input: &str, depth: usize) -> Cow<'_, str> {
    if depth > 4 {
        return Cow::Borrowed(input);
    }
    if let Some((_, en)) = catalog().iter().find(|(zh, _)| zh == input) {
        return Cow::Borrowed(en);
    }
    for (zh, en) in catalog() {
        if let Some(values) = capture(input, zh) {
            let chunks = parts(en);
            if chunks.len() != values.len() + 1 {
                continue;
            }
            let mut output = chunks[0].to_string();
            let mut original = zh.as_str();
            for (value, tail) in values.iter().zip(chunks.iter().skip(1)) {
                let start = original.find('{').unwrap();
                let end = original[start..].find('}').unwrap() + start;
                let name = &original[start + 1..end];
                if matches!(name, "error" | "detail" | "label") {
                    output.push_str(&translate(value, depth + 1));
                } else {
                    output.push_str(value);
                }
                output.push_str(tail);
                original = &original[end + 1..];
            }
            return Cow::Owned(output);
        }
    }
    Cow::Borrowed(input)
}
pub fn text(input: &str) -> Cow<'_, str> {
    if english() {
        translate(input, 0)
    } else {
        Cow::Borrowed(input)
    }
}
/// Used only for existing application labels when the active language changes.
pub fn relabel(input: &str) -> Cow<'_, str> {
    let zh = catalog()
        .iter()
        .find(|(_, en)| en == input)
        .map(|(zh, _)| zh.as_str())
        .unwrap_or(input);
    if english() {
        text(zh)
    } else if zh == input {
        Cow::Borrowed(input)
    } else {
        Cow::Owned(zh.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_uses_windows_ui_language_and_overrides_it() {
        for id in [0x0804, 0x0404, 0x0c04, 0x1404] {
            assert!(!Language::System.english_for(id));
        }
        for id in [0x0409, 0x0411, 0x0407] {
            assert!(Language::System.english_for(id));
        }
        assert!(!Language::Chinese.english_for(0x0409));
        assert!(Language::English.english_for(0x0804));
        assert_eq!(
            serde_json::from_str::<crate::model::Settings>("{}")
                .unwrap()
                .language,
            Language::System
        );
        assert_eq!(
            serde_json::to_string(&Language::Chinese).unwrap(),
            "\"zh-CN\""
        );
    }
    #[test]
    fn catalog_is_complete_and_templates_preserve_values() {
        let mut seen = std::collections::HashSet::new();
        for (zh, en) in catalog() {
            assert!(!en.is_empty());
            assert!(seen.insert(zh));
            assert_eq!(parts(zh).len(), parts(en).len(), "{zh}");
        }
        assert_eq!(
            translate("配置保存失败：\n用户路径 D:\\中文 文件\\mcp.exe", 0),
            "Could not save profile:\n用户路径 D:\\中文 文件\\mcp.exe"
        );
        assert_eq!(
            translate("This is user text 中文", 0),
            "This is user text 中文"
        );
        assert_eq!(
            translate("删除「启动」并停止其网关？", 0),
            "Delete \"启动\" and stop its gateway?"
        );
        assert_eq!(
            translate("当前配置尚未保存。是否先保存？", 0),
            "This profile has unsaved changes. Save them first?"
        );
    }
}
