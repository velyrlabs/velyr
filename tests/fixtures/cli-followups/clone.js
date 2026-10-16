const copy = structuredClone(new Map([[1, 2]]));
console.log(copy.constructor.name, copy.get(1));
