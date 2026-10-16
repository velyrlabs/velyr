import type { Named, Shape } from "./types.ts";

export enum Color {
  Red,
  Green = 5,
}

export function describe(shape: Shape & Named, color: Color): string {
  return `${shape.name}:${shape.sides}:${Color[color]}`;
}
