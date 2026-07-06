use unicode_width::UnicodeWidthChar;

/// Insert a character at the cursor position and return the new cursor.
pub fn insert_at(s: &mut String, cursor: usize, c: char) -> usize {
    let byte_idx = char_boundary(s, cursor);
    s.insert(byte_idx, c);
    cursor + c.width().unwrap_or(0)
}

/// Delete the character before the cursor and return the new cursor.
pub fn backspace_at(s: &mut String, cursor: usize) -> Option<usize> {
    if cursor == 0 {
        return None;
    }
    let byte_idx = char_boundary(s, cursor);
    let prev_byte_idx = char_boundary(s, cursor.saturating_sub(1));
    let n_chars = s[prev_byte_idx..byte_idx].chars().count();
    if n_chars == 0 {
        return None;
    }
    s.drain(prev_byte_idx..byte_idx);
    Some(cursor.saturating_sub(1))
}

/// Delete the word before/at the cursor and return the new cursor.
pub fn delete_word_at(s: &mut String, cursor: usize) -> Option<usize> {
    if cursor == 0 {
        return None;
    }
    let byte_idx = char_boundary(s, cursor);
    // Walk backward through the string to find the word boundary
    let mut boundary = 0;
    let mut seen_word_char = false;
    for (i, ch) in s[..byte_idx].char_indices().rev() {
        if ch.is_alphanumeric() || ch == '_' {
            seen_word_char = true;
        } else if seen_word_char {
            boundary = i + ch.len_utf8();
            break;
        }
        boundary = i;
    }
    if boundary == byte_idx {
        return None;
    }
    s.drain(boundary..byte_idx);
    let char_count = s[..boundary].chars().count();
    Some(char_count)
}

/// Move cursor left by one visual cell.
pub fn cursor_left(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let byte_idx = char_boundary(s, cursor);
    let mut chars = s[..byte_idx].chars().rev();
    let mut new_cursor = cursor;
    if let Some(ch) = chars.next() {
        let w = ch.width().unwrap_or(0);
        new_cursor = new_cursor.saturating_sub(w);
    }
    new_cursor
}

/// Move cursor right by one visual cell.
pub fn cursor_right(s: &str, cursor: usize) -> usize {
    let byte_idx = char_boundary(s, cursor);
    let tail = &s[byte_idx..];
    if let Some(ch) = tail.chars().next() {
        let w = ch.width().unwrap_or(0);
        cursor + w
    } else {
        cursor_end(s, cursor)
    }
}

/// Move cursor to the start of the string (Home).
pub fn cursor_home(_s: &str, _cursor: usize) -> usize {
    0
}

/// Move cursor to the end of the string (End).
pub fn cursor_end(s: &str, _cursor: usize) -> usize {
    s.len()
}

/// Find the byte index of the character at the given visual cursor position.
/// Cursor is measured in cells (visual width), not bytes.
fn char_boundary(s: &str, cursor: usize) -> usize {
    let mut col = 0;
    for (i, ch) in s.char_indices() {
        if col >= cursor {
            return i;
        }
        col += ch.width().unwrap_or(0);
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_at() {
        let mut s = String::from("hello");
        let cursor = insert_at(&mut s, 2, 'x');
        assert_eq!(&s, "hexllo");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn test_backspace_at() {
        let mut s = String::from("hello");
        let cursor = backspace_at(&mut s, 3).unwrap();
        assert_eq!(&s, "helo");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn test_backspace_at_beginning() {
        let mut s = String::from("");
        assert!(backspace_at(&mut s, 0).is_none());
    }

    #[test]
    fn test_delete_word_at() {
        let mut s = String::from("hello world");
        let cursor = delete_word_at(&mut s, 11).unwrap();
        assert_eq!(&s, "hello ");
        assert_eq!(cursor, 6);
    }

    #[test]
    fn test_cursor_left_right() {
        let s = String::from("hi");
        assert_eq!(cursor_left(s.as_str(), 1), 0);
        assert_eq!(cursor_right(s.as_str(), 0), 1);
    }

    #[test]
    fn test_cursor_home_end() {
        let s = String::from("hello");
        assert_eq!(cursor_home(s.as_str(), 3), 0);
        assert_eq!(cursor_end(s.as_str(), 0), s.len());
    }
}
