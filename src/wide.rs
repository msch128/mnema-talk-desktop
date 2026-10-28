//! UTF-16, NUL-terminated, for the Windows wide-character calls.

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::wide;

    #[test]
    fn text_is_utf16_with_one_terminating_nul() {
        assert_eq!(wide("Aé"), [0x41, 0xe9, 0]);
        assert_eq!(wide(""), [0]);
    }
}
