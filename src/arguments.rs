//! Structured arguments trim only boundary whitespace; shell command text is untouched.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn normalize(values: &mut [String]) -> usize {
    let mut changed = 0;
    for value in values {
        let trimmed = value.trim();
        if trimmed.len() != value.len() {
            *value = trimmed.to_owned();
            changed += 1;
        }
    }
    changed
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let mut values = Vec::<String>::deserialize(d)?;
    normalize(&mut values);
    Ok(values)
}

pub fn serialize<S: Serializer>(values: &[String], s: S) -> Result<S::Ok, S::Error> {
    values
        .iter()
        .map(|v| v.trim())
        .collect::<Vec<_>>()
        .serialize(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundary_whitespace_only_and_empty_arguments_preserved() {
        let mut values = [
            " \t\r\n--isolated\r\n ",
            "　msedge\u{a0}",
            " D:/中文 路径/file.txt ",
            " a  b\ninside ",
            " \t\r\n",
            "",
            "\" keep quoted spaces \"",
        ]
        .map(String::from);
        assert_eq!(normalize(&mut values), 5);
        assert_eq!(
            values,
            [
                "--isolated",
                "msedge",
                "D:/中文 路径/file.txt",
                "a  b\ninside",
                "",
                "",
                "\" keep quoted spaces \""
            ]
        );
        assert_eq!(normalize(&mut values), 0);
    }
}
