//! Helpers to read domain enums from rows.

use std::str::FromStr;

use domain::UnknownValue;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

fn parse<T: FromStr<Err = UnknownValue>>(col: &str, s: &str) -> Result<T, sqlx::Error> {
    s.parse()
        .map_err(|e: UnknownValue| sqlx::Error::ColumnDecode {
            index: col.to_owned(),
            source: Box::new(e),
        })
}

/// Reads a non-null enum column.
pub(crate) fn enum_col<T: FromStr<Err = UnknownValue>>(
    row: &SqliteRow,
    col: &str,
) -> Result<T, sqlx::Error> {
    let s: String = row.try_get(col)?;
    parse(col, &s)
}

/// Reads a nullable enum column.
pub(crate) fn opt_enum_col<T: FromStr<Err = UnknownValue>>(
    row: &SqliteRow,
    col: &str,
) -> Result<Option<T>, sqlx::Error> {
    let s: Option<String> = row.try_get(col)?;
    s.map(|s| parse(col, &s)).transpose()
}
