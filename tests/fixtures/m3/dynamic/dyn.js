import { twice } from "./helper.js";

console.log("dyn evaluated");
await Promise.resolve();
export const value = twice(21);
export default "def";
