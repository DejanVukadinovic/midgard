use platform_http::{Problem, internal_error};

pub fn db(e: sqlx::Error) -> Problem {
    internal_error(e)
}

pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

use platform_persistence::sqlx;
