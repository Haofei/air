const { add, multiply } = require("./math");

const sum = add(2, 3);
if (sum !== 5) {
  console.error(
    `examples/code-agent/repair-multifile/math.js:4:10: error: expected add(2, 3) to equal 5, got ${sum}`,
  );
  process.exit(1);
}

const product = multiply(2, 3);
if (product !== 6) {
  console.error(
    `examples/code-agent/repair-multifile/normalize.js:2:10: error: expected multiply(2, 3) to equal 6, got ${product}`,
  );
  process.exit(1);
}

console.log("ok multifile repair fixture");
