import dotenv from 'dotenv';

const parsed = dotenv.parse('A=1\nB="x y"\n# comment\nC=\'single\'\nD=multi\\nline\n');
const expected = { A: '1', B: 'x y', C: 'single', D: 'multi\\nline' };
if (JSON.stringify(parsed) !== JSON.stringify(expected)) throw new Error(JSON.stringify(parsed));
console.log('ok');
