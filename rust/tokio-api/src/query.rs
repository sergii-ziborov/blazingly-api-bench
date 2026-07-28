//! Query-string decoding, written out because there is no extractor to do it.
//!
//! `serde_urlencoded` (what the framework implementations get through their
//! `Query` extractor) is not used here on purpose: this crate exists to show
//! what the request-parsing layer costs when you write it. The one thing it
//! keeps is the behaviour, so `+` is a space, `%XX` is decoded, unknown keys
//! are ignored, and a value that will not parse as the declared type is a 422
//! rather than a silently ignored default.
//!
//! Decoding is lazy: a value with no `%` and no `+` — which is every value in
//! the benchmark scenarios — borrows straight out of the URI with no
//! allocation.

use std::borrow::Cow;

use crate::error::ApiError;

pub struct Pairs<'a> {
    rest: &'a str,
}

pub fn pairs(raw: &str) -> Pairs<'_> {
    Pairs { rest: raw }
}

impl<'a> Iterator for Pairs<'a> {
    type Item = (Cow<'a, str>, Cow<'a, str>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let (pair, rest) = match self.rest.find('&') {
                Some(index) => (&self.rest[..index], &self.rest[index + 1..]),
                None => (self.rest, ""),
            };
            self.rest = rest;
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.find('=') {
                Some(index) => (&pair[..index], &pair[index + 1..]),
                None => (pair, ""),
            };
            return Some((decode(key), decode(value)));
        }
    }
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode(input: &str) -> Cow<'_, str> {
    if !input.as_bytes().iter().any(|byte| *byte == b'%' || *byte == b'+') {
        return Cow::Borrowed(input);
    }
    let raw = input.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        match raw[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < raw.len() => match (hex(raw[index + 1]), hex(raw[index + 2])) {
                (Some(high), Some(low)) => {
                    out.push((high << 4) | low);
                    index += 3;
                }
                _ => {
                    out.push(b'%');
                    index += 1;
                }
            },
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

/// Percent-decoding for a path segment. `+` is a literal plus in a path, not a
/// space, which is the one place the two decoders differ.
pub fn decode_path(input: &str) -> Cow<'_, str> {
    if !input.as_bytes().contains(&b'%') {
        return Cow::Borrowed(input);
    }
    let raw = input.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        match raw[index] {
            b'%' if index + 2 < raw.len() => match (hex(raw[index + 1]), hex(raw[index + 2])) {
                (Some(high), Some(low)) => {
                    out.push((high << 4) | low);
                    index += 3;
                }
                _ => {
                    out.push(b'%');
                    index += 1;
                }
            },
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

pub fn parse_number<T: std::str::FromStr>(field: &str, value: &str) -> Result<T, ApiError> {
    value.parse::<T>().map_err(|_| {
        ApiError::field(
            field,
            "type",
            &format!("`{field}` must be an unsigned integer"),
        )
    })
}
