interface Point { x: number }
function boom(p: Point): never {
  throw new Error("boom " + p.x);
}
try { boom({ x: 1 }); } catch (e) { console.log((e as Error).stack); }
