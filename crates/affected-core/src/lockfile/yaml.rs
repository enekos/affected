//! YAML lockfiles as JSON values, so every format compares records the same way.
use serde_json::{Map, Value};
use yaml_rust2::parser::{EventReceiver, Parser};
use yaml_rust2::scanner::TScalarStyle;
use yaml_rust2::{Event, Yaml};

/// Lockfiles nest a handful of levels; anything deeper is not a lockfile.
const MAX_DEPTH: usize = 32;

/// Every document in `text`; `None` for invalid YAML, aliases, tags, or keys that are not
/// scalars. Lockfiles use none of them, and an alias could expand exponentially.
pub(super) fn documents(text: &str) -> Option<Vec<Value>> {
    let mut builder = Builder::default();
    Parser::new_from_str(text).load(&mut builder, true).ok()?;
    (!builder.failed && builder.stack.is_empty()).then_some(builder.documents)
}

enum Frame {
    Sequence(Vec<Value>),
    Mapping(Map<String, Value>, Option<String>),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    documents: Vec<Value>,
    failed: bool,
}

impl Builder {
    fn open(&mut self, frame: Frame, tagged: bool) {
        if tagged || self.stack.len() >= MAX_DEPTH {
            self.failed = true;
        } else {
            self.stack.push(frame);
        }
    }

    fn close(&mut self) {
        match self.stack.pop() {
            Some(Frame::Sequence(items)) => self.push(Value::Array(items)),
            Some(Frame::Mapping(entries, None)) => self.push(Value::Object(entries)),
            Some(Frame::Mapping(_, Some(_))) | None => self.failed = true,
        }
    }

    fn push(&mut self, value: Value) {
        match self.stack.last_mut() {
            None => self.documents.push(value),
            Some(Frame::Sequence(items)) => items.push(value),
            Some(Frame::Mapping(entries, key)) => match key.take() {
                Some(key) => self.failed |= entries.insert(key, value).is_some(),
                None => match value {
                    Value::String(text) => *key = Some(text),
                    Value::Number(_) | Value::Bool(_) => *key = Some(value.to_string()),
                    _ => self.failed = true,
                },
            },
        }
    }
}

impl EventReceiver for Builder {
    fn on_event(&mut self, event: Event) {
        if self.failed {
            return;
        }
        match event {
            Event::Scalar(text, style, _, tag) => {
                if tag.is_some() {
                    self.failed = true;
                    return;
                }
                let value = if style == TScalarStyle::Plain {
                    match Yaml::from_str(&text) {
                        Yaml::Integer(number) => Value::from(number),
                        Yaml::Boolean(flag) => Value::Bool(flag),
                        Yaml::Null => Value::Null,
                        // Floats stay text: `lockfileVersion: 6.0` must not become `6`.
                        _ => Value::String(text),
                    }
                } else {
                    Value::String(text)
                };
                self.push(value);
            }
            Event::SequenceStart(_, tag) => self.open(Frame::Sequence(Vec::new()), tag.is_some()),
            Event::MappingStart(_, tag) => self.open(Frame::Mapping(Map::new(), None), tag.is_some()),
            Event::SequenceEnd | Event::MappingEnd => self.close(),
            Event::Alias(_) => self.failed = true,
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentStart | Event::DocumentEnd => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_scalars_and_rejects_what_lockfiles_never_contain() {
        let parsed = documents("a: '9.0'\nb: 6.0\nc: 8\n1: x\nd: {e: [true, ~]}\n").unwrap();
        assert_eq!(parsed, [serde_json::json!({"a": "9.0", "b": "6.0", "c": 8, "1": "x", "d": {"e": [true, null]}})]);
        assert_eq!(documents("a: &x 1\nb: *x\n"), None);
        assert_eq!(documents("[a]: 1\n"), None);
        assert_eq!(documents("a: 1\na: 2\n"), None);
        assert_eq!(documents("a: !!str 1\n"), None);
        assert_eq!(documents("a: [\n"), None);
        assert_eq!(documents(&format!("{}1{}", "[".repeat(40), "]".repeat(40))), None);
        assert_eq!(documents("---\na: 1\n---\nb: 2\n").map(|d| d.len()), Some(2));
    }
}
