exports.early = 1;
const b = require("./b.cjs");
exports.done = true;
exports.seenByB = b.sawA;
