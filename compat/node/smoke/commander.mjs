import { Command } from 'commander';

const program = new Command();
program.exitOverride().configureOutput({ writeOut() {}, writeErr() {} });
program.name('smoke').option('-p, --port <n>', 'port').option('-v, --verbose').argument('<file>');
program.parse(['node', 'smoke', '-p', '3', '--verbose', 'a.txt']);
const opts = program.opts();
if (opts.port !== '3' || opts.verbose !== true || program.args[0] !== 'a.txt') throw new Error(JSON.stringify(opts));
let code;
try { new Command().exitOverride().configureOutput({ writeErr() {} }).requiredOption('-x <v>').parse(['node', 'x']); } catch (err) { code = err.code; }
if (code !== 'commander.missingMandatoryOptionValue') throw new Error(`code ${code}`);
console.log('ok');
