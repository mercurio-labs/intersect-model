//! Exact date instants. Lexical calendar/timezone conversion is a separate contract.
use serde::{Deserialize, Serialize};

/// A present EDate instant, measured in milliseconds from the Unix epoch.
///
/// Serialization uses a tagged decimal string to preserve the full signed i64
/// domain in JSON consumers. This type neither reads a clock nor selects a zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "DateWire", into = "DateWire")]
pub struct DateValue {
    milliseconds: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DateWire {
    Date { milliseconds: String },
}

impl DateValue {
    pub const fn new(milliseconds: i64) -> Self {
        Self { milliseconds }
    }

    pub const fn milliseconds(self) -> i64 {
        self.milliseconds
    }
}

impl From<DateValue> for DateWire {
    fn from(value: DateValue) -> Self {
        Self::Date {
            milliseconds: value.milliseconds.to_string(),
        }
    }
}

impl TryFrom<DateWire> for DateValue {
    type Error = String;

    fn try_from(wire: DateWire) -> Result<Self, Self::Error> {
        let DateWire::Date { milliseconds } = wire;
        let parsed = milliseconds
            .parse::<i64>()
            .map_err(|_| "Date milliseconds must be a signed 64-bit integer".to_string())?;
        if parsed.to_string() != milliseconds {
            return Err("Date milliseconds must use canonical ASCII decimal notation".into());
        }
        Ok(Self::new(parsed))
    }
}

/// Explicit calendar selection. Years use astronomical numbering (0 is 1 BCE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calendar {
    ProlepticGregorian,
    /// Julian through 1582-10-04, then Gregorian from 1582-10-15.
    JulianGregorian1582,
}

/// Calendar fields before normalization. Overflowing fields are intentionally
/// accepted by `from_fields`; the resulting instant must still fit signed i64.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateFields {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub millisecond: i64,
}

const DAY: i128 = 86_400_000;
const EPOCH: i128 = 719_163;
const CUTOVER: i128 = 577_736;

fn year_start(year: i128, gregorian: bool) -> i128 {
    let previous = year - 1;
    let base = 365 * previous + previous.div_euclid(4);
    if gregorian {
        base - previous.div_euclid(100) + previous.div_euclid(400) + 1
    } else {
        base - 1
    }
}

fn month_start(year: i128, month: i128, gregorian: bool) -> i128 {
    let leap = year.rem_euclid(4) == 0
        && (!gregorian || year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0);
    let correction = if month <= 2 {
        0
    } else if leap {
        -1
    } else {
        -2
    };
    year_start(year, gregorian) + (367 * month - 362).div_euclid(12) + correction
}

fn offset_millis(minutes: i32) -> Result<i128, String> {
    if !(-1439..=1439).contains(&minutes) {
        return Err("Date offset must be between -23:59 and +23:59".into());
    }
    Ok(i128::from(minutes) * 60_000)
}

impl DateValue {
    /// Normalize calendar fields using an explicit fixed offset east of UTC.
    /// Named zones and DST resolution belong to a separate explicit adapter.
    pub fn from_fields(
        fields: DateFields,
        calendar: Calendar,
        offset_minutes: i32,
    ) -> Result<Self, String> {
        let offset = offset_millis(offset_minutes)?;
        let month_index = i128::from(fields.month) - 1;
        let year = i128::from(fields.year) + month_index.div_euclid(12);
        let month = month_index.rem_euclid(12) + 1;
        let gregorian_day = month_start(year, month, true) + i128::from(fields.day) - 1;
        let day = if calendar == Calendar::JulianGregorian1582 && gregorian_day < CUTOVER {
            month_start(year, month, false) + i128::from(fields.day) - 1
        } else {
            gregorian_day
        };
        let time = ((i128::from(fields.hour) * 60 + i128::from(fields.minute)) * 60
            + i128::from(fields.second))
            * 1000
            + i128::from(fields.millisecond);
        let milliseconds = (day - EPOCH) * DAY + time - offset;
        i64::try_from(milliseconds)
            .map(Self::new)
            .map_err(|_| "Date instant exceeds signed 64-bit milliseconds".into())
    }

    /// Decompose any representable instant without truncating negative epochs.
    pub fn fields(self, calendar: Calendar, offset_minutes: i32) -> Result<DateFields, String> {
        let local = i128::from(self.milliseconds) + offset_millis(offset_minutes)?;
        let day = local.div_euclid(DAY) + EPOCH;
        let time = local.rem_euclid(DAY);
        let gregorian = calendar == Calendar::ProlepticGregorian || day >= CUTOVER;
        // The complete i64 millisecond domain plus an offset fits these bounds.
        let (mut low, mut high) = (-300_000_000i128, 300_000_001i128);
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            if year_start(middle, gregorian) <= day {
                low = middle;
            } else {
                high = middle;
            }
        }
        let year = low;
        let mut month = 1;
        while month < 12 && month_start(year, month + 1, gregorian) <= day {
            month += 1;
        }
        let checked = |value| {
            i64::try_from(value).map_err(|_| "Date field exceeds signed 64-bit range".to_string())
        };
        Ok(DateFields {
            year: checked(year)?,
            month: checked(month)?,
            day: checked(day - month_start(year, month, gregorian) + 1)?,
            hour: checked(time / 3_600_000)?,
            minute: checked(time / 60_000 % 60)?,
            second: checked(time / 1000 % 60)?,
            millisecond: checked(time % 1000)?,
        })
    }
}

/// Deterministic replacement for implicit formatter defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatePolicy {
    pub calendar: Calendar,
    pub default_offset_minutes: i32,
    /// False preserves Ecore's first successfully parsed prefix behavior.
    pub require_full_input: bool,
}

/// A named-zone adapter resolves local fields, including its explicit DST policy.
/// The consumed count is a UTF-8 byte count into the supplied suffix.
pub trait ZoneResolver {
    fn resolve(
        &self,
        suffix: &str,
        local: DateFields,
        calendar: Calendar,
    ) -> Result<Option<(i32, usize)>, String>;
}

/// No named-zone database. Numeric offsets remain available.
pub struct NoNamedZones;
impl ZoneResolver for NoNamedZones {
    fn resolve(&self, _: &str, _: DateFields, _: Calendar) -> Result<Option<(i32, usize)>, String> {
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedDate {
    pub value: DateValue,
    pub consumed: usize,
    /// Index in the pinned Ecore factory's five-format sequence.
    pub format_index: usize,
}

struct Cursor<'a> {
    text: &'a str,
    position: usize,
}
impl Cursor<'_> {
    fn literal(&mut self, literal: &str) -> Option<()> {
        if !self.text.get(self.position..)?.starts_with(literal) {
            return None;
        }
        self.position += literal.len();
        Some(())
    }
    fn number(&mut self) -> Option<i64> {
        while self.text.get(self.position..)?.starts_with([' ', '\t']) {
            self.position += 1;
        }
        let mut normalized = String::new();
        if self.text.get(self.position..)?.starts_with('-') {
            normalized.push('-');
            self.position += 1;
        }
        for character in self.text.get(self.position..)?.chars() {
            let Some(digit) = super::decimal_digit(character) else {
                break;
            };
            normalized.push(digit);
            self.position += character.len_utf8();
        }
        normalized.parse().ok()
    }
}

fn fields_for_format(text: &str, index: usize) -> Option<(DateFields, usize)> {
    let mut cursor = Cursor { text, position: 0 };
    let year = cursor.number()?;
    cursor.literal("-")?;
    let month = cursor.number()?;
    cursor.literal("-")?;
    let day = cursor.number()?;
    let mut fields = DateFields {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        millisecond: 0,
    };
    if index < 4 {
        cursor.literal("T")?;
        fields.hour = cursor.number()?;
        cursor.literal(":")?;
        fields.minute = cursor.number()?;
    }
    if index < 3 {
        cursor.literal(":")?;
        fields.second = cursor.number()?;
    }
    if index < 2 {
        cursor.literal(".")?;
        fields.millisecond = cursor.number()?;
    }
    Some((fields, cursor.position))
}

fn numeric_zone(text: &str) -> Option<(i32, usize)> {
    if text
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GMT"))
    {
        let suffix = text.get(3..)?;
        if !suffix.starts_with(['+', '-']) {
            return Some((0, 3));
        }
        let sign = if suffix.starts_with('-') { -1 } else { 1 };
        let bytes = suffix.as_bytes();
        let mut cursor = 1;
        let mut hour = 0;
        while cursor <= 2 && bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            hour = hour * 10 + i32::from(bytes[cursor] - b'0');
            cursor += 1;
        }
        if cursor == 1 || bytes.get(cursor) != Some(&b':') {
            return None;
        }
        let minute_digits = bytes.get(cursor + 1..cursor + 3)?;
        if !minute_digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let minute = i32::from(minute_digits[0] - b'0') * 10 + i32::from(minute_digits[1] - b'0');
        if hour > 23 || minute > 59 {
            return None;
        }
        return Some((sign * (hour * 60 + minute), 3 + cursor + 3));
    }
    let bytes = text.as_bytes();
    let sign = match bytes.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = bytes.get(1..5)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hour = i32::from(digits[0] - b'0') * 10 + i32::from(digits[1] - b'0');
    let minute = i32::from(digits[2] - b'0') * 10 + i32::from(digits[3] - b'0');
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((sign * (hour * 60 + minute), 5))
}

impl DatePolicy {
    pub fn parse(self, text: &str, zones: &impl ZoneResolver) -> Result<ParsedDate, String> {
        offset_millis(self.default_offset_minutes)?;
        for index in 0..5 {
            let Some((fields, mut consumed)) = fields_for_format(text, index) else {
                continue;
            };
            let mut offset = self.default_offset_minutes;
            if index == 0 {
                let suffix = text.get(consumed..).ok_or("Invalid date cursor")?;
                let zone = match numeric_zone(suffix) {
                    Some(zone) => Some(zone),
                    None => zones.resolve(suffix, fields, self.calendar)?,
                };
                let Some((selected, count)) = zone else {
                    continue;
                };
                if count == 0 || count > suffix.len() || !suffix.is_char_boundary(count) {
                    return Err("Zone resolver returned an invalid consumed span".into());
                }
                offset_millis(selected)?;
                offset = selected;
                consumed += count;
            }
            if self.require_full_input && consumed != text.len() {
                continue;
            }
            return Ok(ParsedDate {
                value: DateValue::from_fields(fields, self.calendar, offset)?,
                consumed,
                format_index: index,
            });
        }
        Err("Input does not match an EDate format under the explicit date policy".into())
    }

    /// Ecore's first format with an explicit fixed offset. Like yyyy, its year
    /// omits the era: BCE values are not guaranteed lexical round trips.
    pub fn format(self, value: DateValue) -> Result<String, String> {
        let fields = value.fields(self.calendar, self.default_offset_minutes)?;
        let year = if fields.year <= 0 {
            1 - fields.year
        } else {
            fields.year
        };
        let sign = if self.default_offset_minutes < 0 {
            '-'
        } else {
            '+'
        };
        let offset = self.default_offset_minutes.abs();
        Ok(format!(
            "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}{sign}{:02}{:02}",
            fields.month,
            fields.day,
            fields.hour,
            fields.minute,
            fields.second,
            fields.millisecond,
            offset / 60,
            offset % 60
        ))
    }
}

/// Stable builtin policy; never consults host defaults or mutable formatter state.
pub const ECORE_POLICY: DatePolicy = DatePolicy {
    calendar: Calendar::JulianGregorian1582,
    default_offset_minutes: 0,
    require_full_input: false,
};

pub(super) fn parse(text: &str) -> Result<serde_json::Value, String> {
    serde_json::to_value(ECORE_POLICY.parse(text, &NoNamedZones)?.value)
        .map_err(|error| error.to_string())
}

pub(super) fn format(value: &serde_json::Value) -> Result<String, String> {
    let instant: DateValue = serde_json::from_value(value.clone())
        .map_err(|error| format!("Invalid date wire: {error}"))?;
    ECORE_POLICY.format(instant)
}
