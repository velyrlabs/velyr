const leaked = Object.getOwnPropertyNames(globalThis).filter(
  (name) => name.startsWith("op_") || name.startsWith("__velyr"),
);
console.log(leaked.length === 0 ? "none" : "leaked: " + leaked.join(","));
