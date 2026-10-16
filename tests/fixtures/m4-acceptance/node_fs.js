// Loading a Node built-in: fails in the web profile, which links no Node compat.
import fs from "node:fs";
console.log(typeof fs.readFileSync);
