const DEFAULT_CONTEXT_MAX_CHARS: usize = 200_000;
const DEFAULT_CONTEXT_THRESHOLD_PERCENT: usize = 80;

pub(crate) fn default_context_budget_chars() -> usize {
    DEFAULT_CONTEXT_MAX_CHARS.saturating_mul(DEFAULT_CONTEXT_THRESHOLD_PERCENT) / 100
}

pub(crate) fn truncate_for_context(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars == 0 {
        return "[AIR_TRUNCATED]".to_string();
    }
    const MARKER: &str = " [AIR_TRUNCATED]";
    if max_chars <= MARKER.chars().count() {
        return MARKER.chars().take(max_chars).collect();
    }
    let keep = max_chars - MARKER.chars().count();
    let mut truncated = value.chars().take(keep).collect::<String>();
    truncated.push_str(MARKER);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_budget_defaults_to_eighty_percent_of_common_model_context() {
        assert_eq!(default_context_budget_chars(), 160_000);
    }

    #[test]
    fn truncation_respects_char_budget_and_marks_output() {
        let value = "abcdef";
        let truncated = truncate_for_context(value, 5);

        assert_eq!(truncated.chars().count(), 5);
        assert_eq!(truncated, " [AIR");
    }

    #[test]
    fn truncation_keeps_marker_when_budget_allows() {
        let value = "x".repeat(128);
        let truncated = truncate_for_context(&value, 32);

        assert_eq!(truncated.chars().count(), 32);
        assert!(truncated.ends_with("[AIR_TRUNCATED]"));
    }
}
