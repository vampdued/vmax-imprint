//! JSON parsing / merging — port of `parse_flexible_json_or_dict` and
//! `merge_translations` from `caption_replacer.py`.
//!
//! The editor accepts the compact `{file: {block_id: text}}` map the app
//! copies "for AI", but users (and LLMs) paste back all kinds of wrappers:
//! ```json / ```python fences, a `x = {...}` assignment, leading/trailing
//! prose, or single quotes. The parser strips those and accepts either
//! strict JSON or a Python-literal dict.

use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

use crate::model::ExtractedData;

/// Strip fences / assignment / surrounding prose, then parse as JSON.
/// Falls back to a Python-literal dict (single quotes, True/False/None).
/// Mirrors the Python implementation, including its failure mode: if
/// neither parse works the error is surfaced to the user.
pub fn parse_flexible_json_or_dict(raw_text: &str) -> Result<Map<String, Value>> {
    let mut txt = raw_text.trim().to_string();

    // ```json ... ``` fences
    if let Some(rest) = txt.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("python"))
            .unwrap_or(rest);
        txt = rest.trim_start().to_string();
    }
    if txt.ends_with("```") {
        txt = txt[..txt.len() - 3].trim_end().to_string();
    }
    // `name = {...}` assignment prefix
    if let Some(eq) = txt.find('=') {
        let before = &txt[..eq];
        if !before.is_empty()
            && before
                .trim()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && txt[eq..]
                .trim_start_matches('=')
                .trim_start()
                .starts_with('{')
        {
            txt = txt[eq + 1..].trim_start().to_string();
        }
    }
    // Keep only the outermost {...}
    if let (Some(first), Some(last)) = (txt.find('{'), txt.rfind('}')) {
        if first <= last {
            txt = txt[first..=last].to_string();
        }
    }

    match serde_json::from_str::<Value>(&txt) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(anyhow!("Top level must be a JSON object / dict.")),
        Err(json_err) => parse_python_literal(&txt).with_context(|| {
            format!("Could not parse JSON ({json_err}); not a Python dict either")
        }),
    }
}

/// Minimal Python-literal dict parser: single-quoted strings, True/False/
/// None, nested dicts. Only what translation maps need — string keys and
/// string (or numeric) values, one level of nesting.
fn parse_python_literal(txt: &str) -> Result<Map<String, Value>> {
    let mut p = PyParser {
        bytes: txt.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.parse_value()?;
    match v {
        Value::Object(map) => Ok(map),
        _ => Err(anyhow!("Top level must be a JSON object / dict.")),
    }
}

struct PyParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> PyParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && (self.bytes[self.pos] as char).is_whitespace() {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn expect(&mut self, c: u8) -> Result<()> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(anyhow!("expected '{}' at byte {}", c as char, self.pos))
        }
    }
    fn parse_value(&mut self) -> Result<Value> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => {
                self.pos += 1;
                let mut map = Map::new();
                loop {
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        self.pos += 1;
                        break;
                    }
                    let key = match self.parse_value()? {
                        Value::String(s) => s,
                        Value::Number(n) => n.to_string(),
                        _ => return Err(anyhow!("dict key must be a string")),
                    };
                    self.expect(b':')?;
                    let val = self.parse_value()?;
                    map.insert(key, val);
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => {
                            self.pos += 1;
                        }
                        Some(b'}') => {}
                        _ => return Err(anyhow!("expected ',' or '}}' in dict")),
                    }
                }
                Ok(Value::Object(map))
            }
            Some(b'[') => {
                self.pos += 1;
                let mut arr = Vec::new();
                loop {
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        self.pos += 1;
                        break;
                    }
                    arr.push(self.parse_value()?);
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => {
                            self.pos += 1;
                        }
                        Some(b']') => {}
                        _ => return Err(anyhow!("expected ',' or ']' in list")),
                    }
                }
                Ok(Value::Array(arr))
            }
            Some(b'"') | Some(b'\'') => {
                let quote = self.peek().unwrap();
                self.pos += 1;
                let mut s = String::new();
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    if c == quote {
                        return Ok(Value::String(s));
                    }
                    if c == b'\\' {
                        if let Some(e) = self.peek() {
                            self.pos += 1;
                            s.push(match e {
                                b'n' => '\n',
                                b't' => '\t',
                                b'r' => '\r',
                                b'\\' => '\\',
                                b'\'' => '\'',
                                b'"' => '"',
                                other => other as char,
                            });
                        }
                    } else {
                        // UTF-8: collect the full char.
                        let start = self.pos - 1;
                        let mut end = start + 1;
                        while end < self.bytes.len() && (self.bytes[end] & 0xC0) == 0x80 {
                            end += 1;
                        }
                        s.push_str(std::str::from_utf8(&self.bytes[start..end]).unwrap_or("�"));
                        self.pos = end;
                    }
                }
                Err(anyhow!("unterminated string"))
            }
            _ => {
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if (c as char).is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'+' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let word = std::str::from_utf8(&self.bytes[start..self.pos])
                    .unwrap_or("")
                    .trim();
                match word {
                    "True" => Ok(Value::Bool(true)),
                    "False" => Ok(Value::Bool(false)),
                    "None" => Ok(Value::Null),
                    "" => Err(anyhow!("unexpected end of input")),
                    _ => {
                        if let Ok(n) = word.parse::<i64>() {
                            Ok(Value::Number(n.into()))
                        } else if let Ok(f) = word.parse::<f64>() {
                            serde_json::Number::from_f64(f)
                                .map(Value::Number)
                                .ok_or_else(|| anyhow!("bad number {word}"))
                        } else {
                            Err(anyhow!("unexpected token {word}"))
                        }
                    }
                }
            }
        }
    }
}

/// Merge a compact `{file: {block_id: new_text}}` translation map into the
/// extracted data (deep copy). Returns `(merged, warnings, changed_count)`.
/// Mirrors Python `merge_translations` exactly, including its warnings.
pub fn merge_translations(
    parsed: &Map<String, Value>,
    extracted: &ExtractedData,
) -> (ExtractedData, Vec<String>, usize) {
    let mut data = extracted.clone();
    let known: std::collections::HashSet<&str> =
        data.images.iter().map(|img| img.file.as_str()).collect();
    let mut warnings: Vec<String> = parsed
        .keys()
        .filter(|k| !known.contains(k.as_str()))
        .map(|k| format!("Skipped \"{k}\" — no matching image in this folder."))
        .collect();
    let mut count = 0usize;

    for img in data.images.iter_mut() {
        let Some(Value::Object(tmap)) = parsed.get(&img.file) else {
            continue;
        };
        let valid_ids: std::collections::HashSet<String> =
            img.blocks.iter().map(|b| b.id.to_string()).collect();
        // Group a file's stale block ids into one line, so three stale
        // translations read as one short sentence instead of three long ones.
        let mut stale: Vec<&String> = tmap.keys().filter(|k| !valid_ids.contains(*k)).collect();
        stale.sort_by(|a, b| match (a.parse::<i64>(), b.parse::<i64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => a.cmp(b),
        });
        if !stale.is_empty() {
            let n = stale.len();
            let ids: Vec<&str> = stale.iter().map(|s| s.as_str()).collect();
            if n == 1 {
                warnings.push(format!(
                    "Skipped 1 translation in {} — block {} isn't in the scan.",
                    img.file, ids[0]
                ));
            } else {
                warnings.push(format!(
                    "Skipped {n} translations in {} — blocks {} aren't in the scan.",
                    img.file,
                    ids.join(", ")
                ));
            }
        }
        for blk in img.blocks.iter_mut() {
            let val = tmap
                .get(&blk.id.to_string())
                .or_else(|| tmap.get(&blk.id.to_string()));
            if let Some(Value::String(s)) = val {
                if !s.trim().is_empty() {
                    blk.new_text = s.clone();
                    if s.trim() != blk.original_text.trim() {
                        count += 1;
                    }
                }
            } else if let Some(v) = val {
                // Non-string values (numbers etc.) are stringified like Python str(val).
                let s = match v {
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                if !s.trim().is_empty() {
                    blk.new_text = s.clone();
                    if s.trim() != blk.original_text.trim() {
                        count += 1;
                    }
                }
            }
        }
    }
    (data, warnings, count)
}

/// The compact map the editor shows / copies for AI:
/// `{file: {block_id: new_text or original_text}}`, only files with blocks.
pub fn compact_map_from_data(data: &ExtractedData) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut compact = BTreeMap::new();
    for img in &data.images {
        let mut blocks_map = BTreeMap::new();
        for blk in &img.blocks {
            let text = if blk.new_text.is_empty() {
                blk.original_text.clone()
            } else {
                blk.new_text.clone()
            };
            blocks_map.insert(blk.id.to_string(), text);
        }
        // An image with no blocks exports `{}` (redesign Phase 6, item 18):
        // the file exists in the map so the AI and the importer know it was
        // seen and is copied unchanged, instead of vanishing from the JSON.
        compact.insert(img.file.clone(), blocks_map);
    }
    compact
}

/// Resolve editor text into renderable data, mirroring Python `_resolve_data`:
/// a full `{"images": [...]}` document is used as-is; otherwise the compact
/// map is merged into the extracted data (loading `captions.json` from the
/// input folder as a fallback when nothing is in memory).
pub fn resolve_data(
    raw: &str,
    extracted: &Option<ExtractedData>,
    input_dir: &std::path::Path,
) -> Result<(ExtractedData, Vec<String>, Option<usize>)> {
    let parsed = parse_flexible_json_or_dict(raw)?;
    if parsed.contains_key("images") {
        let data: ExtractedData = serde_json::from_value(Value::Object(parsed))
            .context("JSON has 'images' but is not a valid captions document")?;
        return Ok((data, Vec::new(), None));
    }
    let base: ExtractedData = match extracted {
        Some(d) => d.clone(),
        None => {
            let fallback = input_dir.join("captions.json");
            let text = std::fs::read_to_string(&fallback).with_context(|| {
                format!(
                    "Run 'Scan & Extract Text' or 'Load captions.json' first (tried {})",
                    fallback.display()
                )
            })?;
            serde_json::from_str(&text).context("captions.json is not valid")?
        }
    };
    let (data, warnings, count) = merge_translations(&parsed, &base);
    Ok((data, warnings, Some(count)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_and_assigned_json() {
        let raw = "Here you go:\n```json\n{\"a.png\": {\"1\": \"Hello\"}}\n```";
        let m = parse_flexible_json_or_dict(raw).unwrap();
        assert_eq!(m["a.png"]["1"], "Hello");
        let raw2 = "data = {'a.png': {'1': 'Hi'}}";
        let m2 = parse_flexible_json_or_dict(raw2).unwrap();
        assert_eq!(m2["a.png"]["1"], "Hi");
    }

    #[test]
    fn merge_counts_changes_and_warns() {
        let mut data = ExtractedData::default();
        data.images.push(crate::model::ImageEntry {
            file: "a.png".into(),
            width: 100,
            height: 100,
            raw_lines: vec![],
            blocks: vec![crate::model::Block {
                id: 1,
                bbox: [0, 0, 10, 10],
                line_bboxes: vec![[0, 0, 10, 10]],
                original_text: "Hello".into(),
                new_text: String::new(),
                confidence: 0.9,
                text_color: [255, 255, 255],
                bg_color: [0, 0, 0],
                is_boxed: true,
                font_size_est: 20,
                kept_override: None,
            }],
            dropped: vec![],
        });
        let parsed = parse_flexible_json_or_dict(
            r#"{"a.png": {"1": "Vanakkam"}, "ghost.png": {"1": "x"}, "a.png2": {}}"#,
        )
        .unwrap();
        // note: the second key is a different file; merge only a.png
        let (merged, warnings, count) = merge_translations(&parsed, &data);
        assert_eq!(merged.images[0].blocks[0].new_text, "Vanakkam");
        assert_eq!(count, 1);
        assert!(warnings.iter().any(|w| w.contains("ghost.png")));
    }

    #[test]
    fn compact_map_keeps_empty_images() {
        let mut data = ExtractedData::default();
        data.images.push(crate::model::ImageEntry {
            file: "empty.png".into(),
            width: 100,
            height: 100,
            raw_lines: vec![],
            blocks: vec![],
            dropped: vec![],
        });
        let compact = compact_map_from_data(&data);
        assert!(compact.contains_key("empty.png"));
        assert!(compact["empty.png"].is_empty());
    }
}
