import named, { alpha, beta } from "./named.cjs";
console.log(alpha, beta(), Object.keys(named).join(","));
