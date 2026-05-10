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

pub(crate) struct ContextFeedbackEntry {
    pub(crate) label: String,
    pub(crate) summary: String,
    pub(crate) output: String,
}

pub(crate) fn recent_context_feedback(
    entries: &[ContextFeedbackEntry],
    omitted_item_name: &str,
) -> String {
    let budget = default_context_budget_chars();
    let mut lines = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;

    for (index, entry) in entries.iter().enumerate().rev() {
        let prefix = format!(
            "- {label}: {summary}; outputs={output_summary}",
            label = entry.label,
            summary = entry.summary,
            output_summary = ""
        );
        let available = budget.saturating_sub(used + prefix.chars().count());
        if available == 0 {
            omitted += 1;
            continue;
        }

        let output_summary = truncate_for_context(&entry.output, available);
        let line = format!(
            "- {label}: {summary}; outputs={output_summary}",
            label = entry.label,
            summary = entry.summary
        );
        used += line.chars().count() + 1;
        lines.push(line);
        if used >= budget {
            omitted += index;
            break;
        }
    }

    if omitted > 0 {
        lines.insert(0, format!(
            "- {omitted} older {omitted_item_name}(s) omitted because AIR context is capped at {budget} chars"
        ));
    }

    truncate_for_context(&lines.join("\n"), budget)
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

    #[test]
    fn recent_feedback_prefers_newer_entries_and_reports_omissions() {
        let entries = vec![
            ContextFeedbackEntry {
                label: "turn 1".to_string(),
                summary: "old".to_string(),
                output: "older".to_string(),
            },
            ContextFeedbackEntry {
                label: "turn 2".to_string(),
                summary: "new".to_string(),
                output: "x".repeat(default_context_budget_chars() + 1024),
            },
        ];

        let feedback = recent_context_feedback(&entries, "turn");

        assert!(feedback.chars().count() <= default_context_budget_chars());
        assert!(feedback.contains("turn 2"));
        assert!(feedback.contains("AIR_TRUNCATED"));
        assert!(feedback.contains("older turn(s) omitted"));
    }
}
