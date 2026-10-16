import { Chalk } from 'chalk';

const color = new Chalk({ level: 1 });
if (color.red('x') !== '\u001b[31mx\u001b[39m') throw new Error(JSON.stringify(color.red('x')));
if (color.bold.underline('y') !== '\u001b[1m\u001b[4my\u001b[24m\u001b[22m') throw new Error(JSON.stringify(color.bold.underline('y')));
if (new Chalk({ level: 0 }).red('z') !== 'z') throw new Error('level 0 must not color');
console.log('ok');
