import pino from 'pino';

const lines = [];
const log = pino({ level: 'info', base: null }, { write: (line) => lines.push(line) });
log.info({ a: 1 }, 'hello %s', 'world');
log.debug('hidden');
const child = log.child({ req: 7 });
child.warn('child');
if (lines.length !== 2) throw new Error(`expected 2 lines, got ${lines.length}`);
const first = JSON.parse(lines[0]);
if (first.msg !== 'hello world' || first.a !== 1 || first.level !== 30) throw new Error(lines[0]);
const second = JSON.parse(lines[1]);
if (second.req !== 7 || second.level !== 40) throw new Error(lines[1]);
console.log('ok');
