import "reflect-metadata";
import { twice } from "./util";
import { twice as twiceJs } from "./util.js";
import { where } from "./dir";
import { Color } from "@/color";
import { twice as twiceTilde } from "~/util";

function track(target: object, key: string): void {
  console.log("paramtypes", Reflect.getMetadata("design:paramtypes", target, key)[0].name);
}

class Service {
  @track
  run(n: number): number {
    return n;
  }
}

console.log("util", twice(2), twiceJs(2), twiceTilde(3));
console.log("dir", where);
console.log("color", Color[Color.Green]);
console.log("dirname", import.meta.dirname.endsWith("src"));
console.log("main", import.meta.main);
console.log(new Service().run(1));
