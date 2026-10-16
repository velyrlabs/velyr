import yargs from 'yargs';

const argv = yargs(['--name=velyr', '-n', '3', '--no-color', 'pos']).parserConfiguration({ 'boolean-negation': true }).parseSync();
if (argv.name !== 'velyr' || argv.n !== 3 || argv.color !== false || argv._[0] !== 'pos') throw new Error(JSON.stringify(argv));
const withCommand = yargs(['serve', '--port', '80'])
  .command('serve', 'serve', (y) => y.option('port', { type: 'number' }))
  .exitProcess(false)
  .parseSync();
if (withCommand._[0] !== 'serve' || withCommand.port !== 80) throw new Error(JSON.stringify(withCommand));
console.log('ok');
