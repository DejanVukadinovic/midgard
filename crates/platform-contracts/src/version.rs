use semver::VersionReq;

#[derive(Debug, thiserror::Error)]
#[error("invalid version constraint: {0}")]
pub struct ConstraintError(pub String);

/// Parse constraints written as `>=1.2.0 <2.0.0` (space- or comma-separated comparators).
pub fn parse_constraint(input: &str) -> Result<VersionReq, ConstraintError> {
    let input = input.trim();
    if input.is_empty() || input.len() > 128 {
        return Err(ConstraintError("empty or too long".into()));
    }
    let mut comparators: Vec<String> = Vec::new();
    let mut pending_op = String::new();
    for tok in input
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
    {
        if tok.chars().all(|c| "<>=~^".contains(c)) {
            pending_op.push_str(tok);
        } else {
            comparators.push(format!("{pending_op}{tok}"));
            pending_op.clear();
        }
    }
    if !pending_op.is_empty() {
        return Err(ConstraintError("dangling operator".into()));
    }
    VersionReq::parse(&comparators.join(", ")).map_err(|e| ConstraintError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    #[test]
    fn parses_range() {
        let r = parse_constraint(">=1.0.0 <2.0.0").unwrap();
        assert!(r.matches(&Version::new(1, 4, 0)));
        assert!(!r.matches(&Version::new(2, 0, 0)));
        let r = parse_constraint(">= 1.1.0, < 2.0.0").unwrap();
        assert!(!r.matches(&Version::new(1, 0, 9)));
        assert!(parse_constraint("garbage").is_err());
        assert!(parse_constraint(">=").is_err());
    }
}
