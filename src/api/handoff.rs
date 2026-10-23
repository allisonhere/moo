//! Framing shared by every surface that forwards one agent's output to another.

pub(crate) const DEFAULT_LINES: u32 = 40;

/// Prompt text for `to`: an optional note followed by `from`'s output, or
/// `None` when there is nothing to hand off.
pub(crate) fn prompt(from: &str, note: Option<&str>, output: &str) -> Option<String> {
    let output = output.trim();
    if output.is_empty() {
        return None;
    }
    let mut text = String::new();
    if let Some(note) = note {
        text.push_str(note.trim());
        text.push_str("\n\n");
    }
    text.push_str(&format!("Output from {from}:\n\n{output}"));
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::prompt;

    #[test]
    fn prompt_frames_output_and_prepends_note() {
        assert_eq!(
            prompt("a", None, " done \n").as_deref(),
            Some("Output from a:\n\ndone")
        );
        assert_eq!(
            prompt("a", Some("review this"), "done").as_deref(),
            Some("review this\n\nOutput from a:\n\ndone")
        );
    }

    #[test]
    fn empty_output_hands_off_nothing() {
        assert_eq!(prompt("a", Some("note"), "  \n"), None);
    }
}
