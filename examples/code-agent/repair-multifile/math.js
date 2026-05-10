const { normalize } = require("./normalize");

function add(a, b) {
  return normalize(a - b);
}

function multiply(a, b) {
  return normalize(a * b);
}

module.exports = { add, multiply };
