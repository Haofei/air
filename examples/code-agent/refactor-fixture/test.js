const { sum } = require("./math");

function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    console.error(
      `examples/code-agent/refactor-fixture/math.js:1:10: error: ${label} expected ${expected}, got ${actual}`
    );
    process.exit(1);
  }
}

assertEqual(sum([1, 2, 3]), 6, "sum([1,2,3])");
assertEqual(sum([]), 0, "sum([])");
console.log("ok");
