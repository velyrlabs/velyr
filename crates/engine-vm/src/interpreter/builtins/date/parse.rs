//! `Date.parse` (ECMA-262 §21.4.3.2): the Date Time String Format, then a fallback for
//! the implementation-defined formats V8 accepts in common use (`toString`/`toUTCString`
//! output, `Feb 29 2020 10:00`, `2/29/2020`, `2020/02/29`, `29 Feb 2020 10:00 UTC`...).

use super::math::{MS_PER_MINUTE, from_fields};

/// A parsed date: an exact time value, or local fields the caller converts with UTC().
pub(crate) enum Parsed {
    Utc(f64),
    Local(f64),
}

pub(crate) fn parse(text: &str) -> Option<Parsed> {
    let text = text.trim();
    iso(text).or_else(|| legacy(text))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn eat(&mut self, byte: u8) -> bool {
        let found = self.bytes.get(self.at) == Some(&byte);
        self.at += usize::from(found);
        found
    }

    /// Exactly `n` digits.
    fn digits(&mut self, n: usize) -> Option<f64> {
        let digits = self.bytes.get(self.at..self.at + n)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.at += n;
        Some(
            digits
                .iter()
                .fold(0.0, |acc, d| acc * 10.0 + f64::from(d - b'0')),
        )
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

/// `YYYY[-MM[-DD]][THH:mm[:ss[.sss]][Z|±HH:mm]]`, with `±YYYYYY` extended years.
fn iso(text: &str) -> Option<Parsed> {
    let mut c = Cursor {
        bytes: text.as_bytes(),
        at: 0,
    };
    let year = match c.bytes.first()? {
        b'+' | b'-' => {
            let negative = c.bytes[0] == b'-';
            c.at = 1;
            let year = c.digits(6)?;
            if negative && year == 0.0 {
                return None;
            }
            if negative { -year } else { year }
        }
        _ => c.digits(4)?,
    };
    let (mut month, mut date) = (1.0, 1.0);
    if c.eat(b'-') {
        month = c.digits(2)?;
        if c.eat(b'-') {
            date = c.digits(2)?;
        }
    }
    if !(1.0..=12.0).contains(&month) || !(1.0..=days_in_month(year, month)).contains(&date) {
        return None;
    }
    let mut time = [0.0; 4];
    let mut offset = Some(0.0);
    if c.eat(b'T') {
        time[0] = c.digits(2)?;
        c.eat(b':').then_some(())?;
        time[1] = c.digits(2)?;
        if c.eat(b':') {
            time[2] = c.digits(2)?;
            if c.eat(b'.') {
                time[3] = fraction_ms(&mut c)?;
            }
        }
        offset = iso_offset(&mut c)?;
    }
    let is_midnight_24 = time[0] == 24.0 && time[1..].iter().all(|&n| n == 0.0);
    if !c.done() || (time[0] > 23.0 && !is_midnight_24) || time[1] > 59.0 || time[2] > 59.0 {
        return None;
    }
    let t = from_fields([year, month - 1.0, date, time[0], time[1], time[2], time[3]]);
    Some(match offset {
        Some(offset) => Parsed::Utc(t - offset),
        None => Parsed::Local(t),
    })
}

/// Milliseconds of `.sss…`; digits past the third are dropped like in V8.
fn fraction_ms(c: &mut Cursor) -> Option<f64> {
    let start = c.at;
    while c.bytes.get(c.at).is_some_and(u8::is_ascii_digit) {
        c.at += 1;
    }
    let digits = &c.bytes[start..c.at];
    if digits.is_empty() {
        return None;
    }
    let padded = digits.iter().chain(b"00").take(3);
    Some(padded.fold(0.0, |acc, d| acc * 10.0 + f64::from(d - b'0')))
}

/// `Some(None)` without an offset (local time), `Some(Some(ms))` with `Z` or `±HH:mm`.
fn iso_offset(c: &mut Cursor) -> Option<Option<f64>> {
    if c.eat(b'Z') {
        return Some(Some(0.0));
    }
    let sign = match c.bytes.get(c.at) {
        Some(b'+') => 1.0,
        Some(b'-') => -1.0,
        _ => return Some(None),
    };
    c.at += 1;
    let hours = c.digits(2)?;
    c.eat(b':').then_some(())?;
    let minutes = c.digits(2)?;
    (hours <= 23.0 && minutes <= 59.0)
        .then_some(Some(sign * (hours * 60.0 + minutes) * MS_PER_MINUTE))
}

fn days_in_month(year: f64, month: f64) -> f64 {
    let leap = (year % 4.0 == 0.0 && year % 100.0 != 0.0) || year % 400.0 == 0.0;
    match month as u8 {
        2 if leap => 29.0,
        2 => 28.0,
        4 | 6 | 9 | 11 => 30.0,
        _ => 31.0,
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Number(f64, usize),
    Word(String),
    Punct(u8),
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let bytes = text.as_bytes();
    let (mut tokens, mut at, mut depth) = (Vec::new(), 0, 0u32);
    while at < bytes.len() {
        let byte = bytes[at];
        let start = at;
        at += 1;
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.checked_sub(1)?,
            _ if depth > 0 => {}
            b'0'..=b'9' => {
                while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
                let value = text[start..at].parse().ok()?;
                tokens.push(Token::Number(value, at - start));
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
                    at += 1;
                }
                tokens.push(Token::Word(text[start..at].to_ascii_lowercase()));
            }
            b' ' | b',' | b'\t' | b'\n' | b'\r' => {}
            b'+' | b'-' | b':' | b'/' | b'.' => tokens.push(Token::Punct(byte)),
            _ => return None,
        }
    }
    Some(tokens)
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// Offset in minutes of the US zone abbreviations V8 knows.
fn zone_minutes(word: &str) -> Option<f64> {
    Some(match word {
        "z" | "ut" | "utc" | "gmt" => 0.0,
        "edt" => -240.0,
        "est" | "cdt" => -300.0,
        "cst" | "mdt" => -360.0,
        "mst" | "pdt" => -420.0,
        "pst" => -480.0,
        _ => return None,
    })
}

#[derive(Default)]
struct Legacy {
    numbers: Vec<(f64, usize)>,
    month: Option<f64>,
    time: Vec<f64>,
    pm: Option<bool>,
    offset: Option<f64>,
    slash_or_dash: bool,
}

fn legacy(text: &str) -> Option<Parsed> {
    let tokens = tokenize(text)?;
    // A leading sign is ISO's extended year; V8 rejects it outside that format.
    if matches!(tokens.first(), Some(Token::Punct(_))) {
        return None;
    }
    let mut state = Legacy::default();
    let mut i = 0;
    while i < tokens.len() {
        i = state.token(&tokens, i)?;
    }
    state.finish()
}

impl Legacy {
    /// Consumes the token at `i` (and what it governs); returns the next index.
    fn token(&mut self, tokens: &[Token], i: usize) -> Option<usize> {
        match &tokens[i] {
            Token::Number(..) if tokens.get(i + 1) == Some(&Token::Punct(b':')) => {
                self.time_of_day(tokens, i)
            }
            Token::Number(n, len) => {
                self.numbers.push((*n, *len));
                Some(i + 1)
            }
            Token::Word(word) => self.word(word, i),
            Token::Punct(sign @ (b'+' | b'-'))
                if self.offset.is_some() || !self.time.is_empty() =>
            {
                let sign = if *sign == b'-' { -1.0 } else { 1.0 };
                self.zone_offset(sign, tokens, i + 1)
            }
            Token::Punct(b'/' | b'-') => {
                self.slash_or_dash = true;
                Some(i + 1)
            }
            Token::Punct(b'.') => Some(i + 1),
            Token::Punct(_) => None,
        }
    }

    fn time_of_day(&mut self, tokens: &[Token], mut i: usize) -> Option<usize> {
        if !self.time.is_empty() {
            return None;
        }
        while let Some(Token::Number(n, _)) = tokens.get(i) {
            self.time.push(*n);
            i += 1;
            match tokens.get(i) {
                Some(Token::Punct(b':')) if self.time.len() < 3 => i += 1,
                Some(Token::Punct(b'.')) if self.time.len() == 3 => i += 1,
                _ => break,
            }
        }
        Some(i)
    }

    fn word(&mut self, word: &str, i: usize) -> Option<usize> {
        let prefix = word.get(..3).unwrap_or(word);
        if let Some(month) = MONTHS.iter().position(|m| *m == prefix) {
            self.month = Some(month as f64);
        } else if word == "am" || word == "pm" {
            self.pm = Some(word == "pm");
        } else if let Some(minutes) = zone_minutes(word) {
            self.offset = Some(minutes);
        } else if word == "t" || WEEKDAYS.contains(&prefix) {
        } else if self.numbers.is_empty() && self.month.is_none() {
            // V8 ignores unknown words before the date starts.
        } else {
            return None;
        }
        Some(i + 1)
    }

    /// `+hhmm`, `+hh:mm` or `+hh` after a zone or a time.
    fn zone_offset(&mut self, sign: f64, tokens: &[Token], i: usize) -> Option<usize> {
        let Some(Token::Number(n, len)) = tokens.get(i) else {
            return None;
        };
        let (minutes, next) = match (len, tokens.get(i + 1), tokens.get(i + 2)) {
            (1 | 2, Some(Token::Punct(b':')), Some(Token::Number(m, _))) => (n * 60.0 + m, i + 3),
            (1 | 2, _, _) => (n * 60.0, i + 1),
            _ => ((n / 100.0).floor() * 60.0 + n % 100.0, i + 1),
        };
        self.offset = Some(self.offset.unwrap_or(0.0) + sign * minutes);
        Some(next)
    }

    fn finish(self) -> Option<Parsed> {
        let (year, month, date) = self.date()?;
        let mut time = self.time.clone();
        time.resize(4, 0.0);
        let [mut hour, minute, second, ms] = [time[0], time[1], time[2], time[3]];
        if let Some(pm) = self.pm {
            if hour > 12.0 {
                return None;
            }
            hour = hour % 12.0 + if pm { 12.0 } else { 0.0 };
        }
        if hour > 24.0 || minute > 59.0 || second > 59.0 || !(1.0..=31.0).contains(&date) {
            return None;
        }
        let t = from_fields([year, month, date, hour, minute, second, ms]);
        Some(match self.offset {
            Some(minutes) => Parsed::Utc(t - minutes * MS_PER_MINUTE),
            None => Parsed::Local(t),
        })
    }

    /// (year, month 0-11, date) from the numbers seen outside the time of day.
    fn date(&self) -> Option<(f64, f64, f64)> {
        let numbers = &self.numbers;
        let (year, month, date) = match (self.month, numbers.as_slice()) {
            // `Feb 29 2020`, `29 Feb 2020`, `2020 Feb 29`
            (Some(month), [(a, _), (b, blen)]) if *blen < 3 && *a > 31.0 => (*a, month, *b),
            (Some(month), [(a, _), (b, _)]) => (*b, month, *a),
            (Some(month), [(a, _)]) => (*a, month, 1.0),
            // `2020/02/29`, `2020-02-29 10:00`
            (None, [(y, 4..), (m, _), (d, _)]) if self.slash_or_dash => (*y, m - 1.0, *d),
            // `2/29/2020`
            (None, [(m, _), (d, _), (y, _)]) if self.slash_or_dash => (*y, m - 1.0, *d),
            _ => return None,
        };
        if !(0.0..12.0).contains(&month) {
            return None;
        }
        let year = match (year, numbers.iter().any(|(n, len)| *n == year && *len <= 2)) {
            (y, true) if y < 50.0 => y + 2000.0,
            (y, true) if y < 100.0 => y + 1900.0,
            (y, _) => y,
        };
        Some((year, month, date))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> Option<f64> {
        match parse(text)? {
            Parsed::Utc(t) => Some(t),
            Parsed::Local(_) => None,
        }
    }

    #[test]
    fn iso_and_rfc_strings_parse_to_utc() {
        assert_eq!(utc("1970-01-01T00:00:00.007Z"), Some(7.0));
        assert_eq!(
            utc("Thu, 01 Jan 1970 00:00:00 GMT+0100"),
            Some(-3_600_000.0)
        );
        assert_eq!(utc("Thu Jan 01 1970 01:00:00 GMT+0100 (CET)"), Some(0.0));
        // V8 rolls `2020-02-30` over to March 1 through its legacy parser.
        assert!(iso("2020-02-30").is_none() && parse("-000000-01-01").is_none());
        assert!(matches!(parse("2020-02-29T00:00"), Some(Parsed::Local(_))));
    }
}
