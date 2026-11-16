//! Timestamp storage format.
//!
//! Timestamps are stored as UTC text `YYYY-MM-DDTHH:MM:SS.sssZ`: RFC 3339,
//! fixed width, so comparing strings in SQL compares times. Sub-millisecond
//! precision is truncated. Reads use sqlx's chrono decoding, which accepts
//! this form.

use chrono::{DateTime, Utc};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{SqliteArgumentsBuffer, SqliteTypeInfo};
use sqlx::{Encode, Sqlite, Type};

/// Formats a time in the storage format.
pub fn format_ts(t: &DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// A bind parameter that writes a time in the storage format. Always bind
/// times through this, never `DateTime` directly: sqlx's own encoding is not
/// fixed width and the schema rejects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ts(pub DateTime<Utc>);

impl Ts {
    pub fn opt(t: Option<DateTime<Utc>>) -> Option<Ts> {
        t.map(Ts)
    }
}

impl Type<Sqlite> for Ts {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }
}

impl Encode<'_, Sqlite> for Ts {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        Encode::<Sqlite>::encode(format_ts(&self.0), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn format_is_fixed_width_utc() {
        let t = Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
        assert_eq!(format_ts(&t), "2026-03-04T05:06:07.000Z");
        let t = t + chrono::Duration::nanoseconds(123_456_789);
        assert_eq!(format_ts(&t), "2026-03-04T05:06:07.123Z");
    }
}
