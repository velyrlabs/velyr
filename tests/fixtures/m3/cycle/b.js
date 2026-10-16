import { getA } from "./a.js";

export const b = "b";
console.log("b sees getA as", typeof getA);
try {
  getA();
} catch (error) {
  console.log("a is in TDZ:", error.name);
}
