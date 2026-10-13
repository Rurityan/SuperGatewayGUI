//! Bounded, already-redacted log records, identified by stable service ID.
use std::collections::VecDeque;

#[derive(Default)]
pub struct Logs {
    entries: VecDeque<(String, String)>,
    bytes: usize,
}
impl Logs {
    pub fn push(&mut self, id: String, text: String) {
        self.bytes += id.len() + text.len();
        self.entries.push_back((id, text));
        while self.entries.len() > 1000 || self.bytes > 128 * 1024 {
            if let Some((id, text)) = self.entries.pop_front() {
                self.bytes -= id.len() + text.len();
            }
        }
    }
    pub fn text(&self, service: Option<&str>) -> String {
        self.entries
            .iter()
            .filter(|(id, _)| service.is_none_or(|selected| selected == id))
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
    }
    pub fn clear(&mut self, service: Option<&str>) {
        self.entries
            .retain(|(id, _)| service.is_some_and(|selected| selected != id));
        self.bytes = self
            .entries
            .iter()
            .map(|(id, text)| id.len() + text.len())
            .sum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_by_id_and_keeps_interleaved_history() {
        let mut logs = Logs::default();
        logs.push("a".into(), "[同名] a1".into());
        logs.push("b".into(), "[同名] b1".into());
        logs.push("a".into(), "[改名] a2".into());
        assert_eq!(logs.text(Some("a")), "[同名] a1\r\n[改名] a2");
        assert_eq!(logs.text(Some("b")), "[同名] b1");
        assert_eq!(logs.text(None), "[同名] a1\r\n[同名] b1\r\n[改名] a2");
        logs.clear(Some("a"));
        assert_eq!(logs.text(None), "[同名] b1");
        logs.clear(None);
        assert!(logs.text(None).is_empty());
    }
    #[test]
    fn capacity_remains_bounded_across_filters_and_clear() {
        let mut logs = Logs::default();
        for i in 0..1100 {
            logs.push("a".into(), i.to_string());
        }
        assert_eq!(logs.entries.len(), 1000);
        assert!(logs.text(None).starts_with("100\r\n"));
        logs.clear(Some("a"));
        logs.push("b".into(), "x".repeat(128 * 1024));
        assert!(logs.entries.is_empty() && logs.bytes == 0);
        logs.push("b".into(), "new".into());
        assert_eq!(logs.text(Some("b")), "new");
    }
}
