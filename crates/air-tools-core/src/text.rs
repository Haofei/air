use crate::TruncationDirection;

pub fn select_line_range(
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> String {
    if start_line.is_none() && end_line.is_none() {
        return content.to_string();
    }
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or(usize::MAX);
    content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + 1;
            (line_number >= start && line_number <= end).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn merge_line_ranges(
    match_lines: &[usize],
    total_lines: usize,
    context_lines: usize,
) -> Vec<(usize, usize)> {
    let mut lines = match_lines.to_vec();
    lines.sort_unstable();
    lines.dedup();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for line in lines {
        let start = line.saturating_sub(context_lines).max(1);
        let end = line.saturating_add(context_lines).min(total_lines.max(1));
        match ranges.last_mut() {
            Some((_, previous_end)) if start <= previous_end.saturating_add(1) => {
                *previous_end = (*previous_end).max(end);
            }
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

pub fn numbered_line_range(content: &str, start_line: usize, end_line: usize) -> String {
    content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = index + 1;
            (line_number >= start_line && line_number <= end_line)
                .then(|| format!("{line_number}: {line}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn bytes_to_limited_text(bytes: &[u8], max_bytes: usize) -> (String, bool, usize) {
    bytes_to_limited_text_with_direction(bytes, max_bytes, TruncationDirection::Head)
}

pub fn bytes_to_limited_text_with_direction(
    bytes: &[u8],
    max_bytes: usize,
    direction: TruncationDirection,
) -> (String, bool, usize) {
    let truncated = bytes.len() > max_bytes;
    let limited = if !truncated {
        bytes
    } else if matches!(direction, TruncationDirection::Tail) {
        &bytes[bytes.len() - max_bytes..]
    } else {
        &bytes[..max_bytes]
    };
    (
        String::from_utf8_lossy(limited).to_string(),
        truncated,
        limited.len(),
    )
}
