pub fn apply_discount(price_cents: u32, discount_percent: u32) -> u32 {
    let discount = discount_percent.min(100);
    price_cents.saturating_sub(price_cents * discount / 100)
}
