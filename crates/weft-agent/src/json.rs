//! Minimal JSON reader and writer.
//!
//! Providers speak JSON, but the agent core must stay dependency free while the
//! network is unavailable. This module implements just the JSON grammar:
//! values, strings with escapes (including surrogate pairs), numbers, arrays
//! and objects. It is strict: trailing data, control characters inside strings
//! and lone surrogates are errors instead of being silently accepted.

use std::fmt;

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, stored as `f64`.
    Number(f64),
    /// A string with escapes already resolved.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object; order is preserved.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Looks up a key in an object.
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// Looks up an index in an array.
    pub fn index(&self, index: usize) -> Option<&Self> {
        match self {
            Self::Array(items) => items.get(index),
            _ => None,
        }
    }

    /// The value as a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    /// The value as a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// Serializes the value back to compact JSON.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out);
        out
    }
}

/// Why parsing failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonError {
    /// Character offset where the parser stopped.
    pub offset: usize,
    /// What was expected or found.
    pub reason: &'static str,
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "JSON error at {}: {}", self.offset, self.reason)
    }
}

impl std::error::Error for JsonError {}

/// Parses a complete JSON document.
pub fn parse(input: &str) -> Result<Value, JsonError> {
    let mut parser = Parser {
        chars: input.chars().collect(),
        position: 0,
    };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.position != parser.chars.len() {
        return Err(parser.error("trailing data"));
    }
    Ok(value)
}

/// Escapes a string and wraps it in quotes.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    write_string(text, &mut out);
    out
}

struct Parser {
    chars: Vec<char>,
    position: usize,
}

impl Parser {
    fn error(&self, reason: &'static str) -> JsonError {
        JsonError {
            offset: self.position,
            reason,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.position += 1;
        Some(c)
    }

    fn expect(&mut self, expected: char) -> Result<(), JsonError> {
        match self.next() {
            Some(c) if c == expected => Ok(()),
            Some(_) => Err(self.error("unexpected character")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.position += 1;
        }
    }

    fn value(&mut self) -> Result<Value, JsonError> {
        self.skip_whitespace();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => self.string().map(Value::String),
            Some('t') => self.literal("true", Value::Bool(true)),
            Some('f') => self.literal("false", Value::Bool(false)),
            Some('n') => self.literal("null", Value::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.error("unexpected value")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, JsonError> {
        for expected in word.chars() {
            self.expect(expected)?;
        }
        Ok(value)
    }

    fn object(&mut self) -> Result<Value, JsonError> {
        self.expect('{')?;
        let mut entries = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.position += 1;
            return Ok(Value::Object(entries));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(':')?;
            let value = self.value()?;
            entries.push((key, value));
            self.skip_whitespace();
            match self.next() {
                Some(',') => {}
                Some('}') => return Ok(Value::Object(entries)),
                Some(_) => return Err(self.error("expected , or }")),
                None => return Err(self.error("unexpected end of input")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, JsonError> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.position += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.next() {
                Some(',') => {}
                Some(']') => return Ok(Value::Array(items)),
                Some(_) => return Err(self.error("expected , or ]")),
                None => return Err(self.error("unexpected end of input")),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.next() {
                Some('"') => return Ok(out),
                Some('\\') => self.escape(&mut out)?,
                Some(c) if (c as u32) < 0x20 => {
                    return Err(self.error("control character in string"));
                }
                Some(c) => out.push(c),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        let escaped = self
            .next()
            .ok_or_else(|| self.error("unterminated escape"))?;
        let c = match escaped {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => return self.unicode(out),
            _ => return Err(self.error("invalid escape")),
        };
        out.push(c);
        Ok(())
    }

    fn unicode(&mut self, out: &mut String) -> Result<(), JsonError> {
        let high = self.hex4()?;
        let code = if (0xD800..=0xDBFF).contains(&high) {
            self.expect('\\')?;
            self.expect('u')?;
            let low = self.hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(self.error("invalid low surrogate"));
            }
            0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
        } else if (0xDC00..=0xDFFF).contains(&high) {
            return Err(self.error("lone low surrogate"));
        } else {
            high
        };
        let c = char::from_u32(code).ok_or_else(|| self.error("invalid code point"))?;
        out.push(c);
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let mut value = 0;
        for _ in 0..4 {
            let c = self
                .next()
                .ok_or_else(|| self.error("short unicode escape"))?;
            let digit = c
                .to_digit(16)
                .ok_or_else(|| self.error("invalid hex digit"))?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.position;
        if self.peek() == Some('-') {
            self.position += 1;
        }
        while self.peek().is_some_and(|c| {
            c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-'
        }) {
            self.position += 1;
        }
        let text: String = self.chars[start..self.position].iter().collect();
        text.parse::<f64>()
            .map(Value::Number)
            .map_err(|_| self.error("invalid number"))
    }
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let code = c as u32;
                out.push_str("\\u");
                out.push(HEX[((code >> 12) & 0xF) as usize] as char);
                out.push(HEX[((code >> 8) & 0xF) as usize] as char);
                out.push(HEX[((code >> 4) & 0xF) as usize] as char);
                out.push(HEX[(code & 0xF) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_documents() {
        let value = parse(r#"{"a":[1,true,null,"x"],"b":{"c":-2.5}}"#).unwrap();
        assert_eq!(
            value.get("a").unwrap().index(0).unwrap().as_f64(),
            Some(1.0)
        );
        assert_eq!(value.get("a").unwrap().index(2), Some(&Value::Null));
        assert_eq!(
            value.get("b").unwrap().get("c").unwrap().as_f64(),
            Some(-2.5)
        );
    }

    #[test]
    fn resolves_escapes_and_surrogates() {
        let value = parse(r#""a\n\"b\"\u00f1\uD83D\uDE00""#).unwrap();
        assert_eq!(value.as_str(), Some("a\n\"b\"ñ😀"));
    }

    #[test]
    fn writes_compact_json_with_escapes() {
        let value = Value::Object(vec![
            ("text".to_owned(), Value::String("a\n\"b\"".to_owned())),
            ("n".to_owned(), Value::Number(2.0)),
        ]);
        assert_eq!(value.to_json(), r#"{"text":"a\n\"b\"","n":2}"#);
    }

    #[test]
    fn rejects_trailing_data_and_control_characters() {
        assert!(parse("{} x").is_err());
        assert!(parse("\"a\nb\"").is_err());
    }

    #[test]
    fn rejects_lone_surrogates() {
        assert!(parse(r#""\uD800""#).is_err());
        assert!(parse(r#""\uDC00""#).is_err());
    }
}
