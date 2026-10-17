function log(value: unknown, ctx: { kind: string; name: string | symbol }): void {
  console.log("decorated", ctx.kind, String(ctx.name));
}
class Box {
  @log accessor v = 1;
}
const box = new Box();
box.v = 5;
console.log(box.v);
