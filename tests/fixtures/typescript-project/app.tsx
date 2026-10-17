enum Level { Low, High }
const h = (tag: string, props: unknown): string => `<${tag}/>`;
console.log(<i />, Level[Level.High]);
