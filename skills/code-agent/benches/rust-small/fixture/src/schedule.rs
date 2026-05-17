pub fn normalized_window(start: u32, end: u32) -> (u32, u32) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}
