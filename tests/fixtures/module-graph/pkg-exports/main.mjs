import p from "p";
import { createRequire } from "node:module";
console.log(p, createRequire(import.meta.url)("p"));
