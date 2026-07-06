//! Tab display formatting — configurable layout strings.

/// Recognized format identifiers in tab display layout strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatIdent {
    /// The tab's current position-based index (1-based).
    Index,
    /// The tab's custom name (if set).
    Name,
    /// A literal string segment.
    Literal(String),
}

/// Errors that can occur when parsing a tab display layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabDisplayError {
    UnknownIdent(String),
    UnclosedBrace,
}

impl std::fmt::Display for TabDisplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownIdent(ident) => write!(f, "unknown format identifier: {}", ident),
            Self::UnclosedBrace => write!(f, "unclosed brace in tab display layout"),
        }
    }
}

impl std::error::Error for TabDisplayError {}

/// Parse a format string like `"{index}: {name}"` into segments.
pub fn parse_tab_layout(layout: &str) -> Result<Vec<FormatIdent>, TabDisplayError> {
    let mut segments: Vec<FormatIdent> = Vec::new();
    let mut literal_start: usize = 0;
    let mut chars = layout.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '{' {
            let mut ident: String = String::new();
            let mut found_close: bool = false;
            while let Some((_k, c)) = chars.peek() {
                if *c == '}' {
                    found_close = true;
                    chars.next();
                    break;
                }
                ident.push(*c);
                chars.next();
            }
            if !found_close {
                return Err(TabDisplayError::UnclosedBrace);
            }
            if i > literal_start {
                segments.push(FormatIdent::Literal(layout[literal_start..i].to_string()));
            }
            match ident.as_str() {
                "index" => segments.push(FormatIdent::Index),
                "name" => segments.push(FormatIdent::Name),
                other => return Err(TabDisplayError::UnknownIdent(other.to_string())),
            }
            literal_start = i + ident.len() + 2;
        }
    }
    if literal_start < layout.len() {
        segments.push(FormatIdent::Literal(layout[literal_start..].to_string()));
    }
    if segments.is_empty() {
        segments.push(FormatIdent::Index);
    }
    Ok(segments)
}

/// Resolve parsed segments against a tab's position and optional name.
///
/// When `custom_name` is `None`, `Name` segments are skipped (omitting
/// separators) and `Index` segments still render. This ensures unnamed
/// tabs never show dangling text like `"3: "` or `"3 - foo"`.
pub fn format_tab(segments: &[FormatIdent], index: usize, custom_name: Option<&str>) -> String {
    // If unnamed and there's a Name segment anywhere, render just the index
    // to avoid dangling separators (e.g. `"3: "` from `"{index}: {name}"`).
    if custom_name.is_none() && segments.iter().any(|s| matches!(s, FormatIdent::Name)) {
        return index.to_string();
    }

    let mut out = String::new();
    for seg in segments {
        match seg {
            FormatIdent::Index => out.push_str(&index.to_string()),
            FormatIdent::Name => {
                if let Some(name) = custom_name {
                    out.push_str(name);
                }
            }
            FormatIdent::Literal(s) => out.push_str(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_index_only() {
        let segs = parse_tab_layout("{index}").unwrap();
        assert_eq!(segs, vec![FormatIdent::Index]);
    }

    #[test]
    fn parse_index_name() {
        let segs = parse_tab_layout("{index}: {name}").unwrap();
        assert_eq!(segs, vec![
            FormatIdent::Index,
            FormatIdent::Literal(": ".to_string()),
            FormatIdent::Name,
        ]);
    }

    #[test]
    fn format_unnamed_tab() {
        let segs = parse_tab_layout("{index}: {name}").unwrap();
        assert_eq!(format_tab(&segs, 3, None), "3");
        assert_eq!(format_tab(&segs, 3, Some("foo")), "3: foo");
    }

    #[test]
    fn empty_layout_defaults_to_index() {
        let segs = parse_tab_layout("").unwrap();
        assert_eq!(segs, vec![FormatIdent::Index]);
        assert_eq!(format_tab(&segs, 1, Some("test")), "1");
    }
}
