const lib = require("./lib");
const data = require("./data.json");
console.log(lib.add(2, 3), data.name, typeof module, typeof __filename);
