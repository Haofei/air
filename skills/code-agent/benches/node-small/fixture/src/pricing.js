export function applyDiscount(priceCents, discountPercent) {
  const discount = Math.min(Math.max(discountPercent, 0), 100);
  return priceCents - Math.floor((priceCents * discount) / 100);
}
