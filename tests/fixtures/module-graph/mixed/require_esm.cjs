const ns = require("./esm.mjs");
console.log(Object.keys(ns).sort().join(","), ns.x, ns.default, require("./data.json").k);
