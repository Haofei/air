#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub archived: bool,
    pub score: u8,
}

pub fn active_names(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter(|item| !item.archived && item.score > 0)
        .map(|item| item.name.clone())
        .collect()
}
