const { add } = require("./math");

const actual = add(2, 3);
if (actual !== 5) {
  console.error(
    `examples/code-agent/edit-fixture/math.js:2:10: error: expected add(2, 3) to equal 5, got ${actual}`,
  );
  process.exit(1);
}

console.log("ok edit fixture");
