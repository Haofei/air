import assert from "node:assert/strict";
import test from "node:test";
import { statusLabel } from "../src/http.js";
import { applyDiscount } from "../src/pricing.js";
import { slugify } from "../src/slug.js";
import { displayName } from "../src/users.js";

test("slugify normalizes words", () => {
  assert.equal(slugify(" Hello, AIR Runtime "), "hello-air-runtime");
});

test("applyDiscount clamps discounts", () => {
  assert.equal(applyDiscount(10_000, 25), 7_500);
  assert.equal(applyDiscount(10_000, 250), 0);
  assert.equal(applyDiscount(10_000, -20), 10_000);
});

test("statusLabel classifies common ranges", () => {
  assert.equal(statusLabel(204), "ok");
  assert.equal(statusLabel(404), "client_error");
  assert.equal(statusLabel(503), "server_error");
  assert.equal(statusLabel(302), "unknown");
});

test("displayName appends initials", () => {
  assert.equal(
    displayName({ firstName: "Ada", lastName: "Lovelace" }),
    "Ada Lovelace (AL)",
  );
});
