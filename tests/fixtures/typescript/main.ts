import { Color, describe } from "./lib.ts";
import type { Shape } from "./types.ts";

const square: Shape & { name: string } = { name: "square", sides: 4 };
console.log(describe(square, Color.Green));
console.log(Color.Red, Color[5]);
