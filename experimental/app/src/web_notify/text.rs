//! The plain text of a toast, a server's name or any other text the host
//! shows from a page or a server. Shared with the mobile host (`mobile/`).

/// Invisible format characters (Unicode `Cf`) that reorder or hide text:
/// zero-width spaces and marks, bidi overrides and isolates, the word joiner,
/// the byte order mark and the tag characters (which spell ASCII text that no
/// one sees). The zero-width joiner stays (emoji sequences need it) and so does the
/// zero-width non-joiner (Persian words need it).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}' | '\u{200e}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}'
            | '\u{feff}' | '\u{e0000}'..='\u{e007f}'
    )
}

/// Plain text only: control characters become spaces, invisible format
/// characters are dropped, the length is bounded in characters, and anything
/// cut off is marked.
pub fn clean(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .filter(|c| !is_invisible(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}
